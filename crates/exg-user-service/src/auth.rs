use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng},
};
use hmac::{Hmac, Mac};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation};
use rustc_hash::FxHashMap;
use sha2::Sha256;
use uuid::Uuid;

use exg_common::{AccountId, UnixMicros, UserId};

use crate::error::AuthError;
use crate::user::{ApiKey, ApiPermissions, KycLevel, SubAccount, User};

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct JwtClaims {
    pub sub: u64,
    pub exp: u64,
    pub iat: u64,
    pub jti: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LoginResponse {
    pub token: String,
    pub expires_at: u64,
    pub user_id: UserId,
}

pub struct AuthService {
    jwt_secret: Vec<u8>,
    jwt_expiry_secs: u64,
    users: FxHashMap<UserId, User>,
    users_by_email: FxHashMap<String, UserId>,
    api_keys: FxHashMap<String, ApiKey>,
    sub_accounts: FxHashMap<AccountId, SubAccount>,
    next_user_id: u64,
    next_account_id: u64,
}

impl AuthService {
    pub fn new(jwt_secret: &[u8], jwt_expiry_secs: u64) -> Self {
        Self {
            jwt_secret: jwt_secret.to_vec(),
            jwt_expiry_secs,
            users: FxHashMap::default(),
            users_by_email: FxHashMap::default(),
            api_keys: FxHashMap::default(),
            sub_accounts: FxHashMap::default(),
            next_user_id: 1,
            next_account_id: 1,
        }
    }

    /// Register a new user. Returns user_id.
    pub fn register(&mut self, email: &str, password: &str) -> Result<UserId, AuthError> {
        validate_password(password)?;

        if self.users_by_email.contains_key(email) {
            return Err(AuthError::EmailAlreadyExists);
        }

        let password_hash = hash_password(password)?;
        let user_id = UserId::new(self.next_user_id);
        self.next_user_id += 1;

        let now = UnixMicros::now();
        let user = User {
            user_id,
            email: email.to_string(),
            password_hash,
            totp_secret: None,
            kyc_level: KycLevel::L0,
            is_active: true,
            created_at: now,
            updated_at: now,
        };

        self.users_by_email.insert(email.to_string(), user_id);
        self.users.insert(user_id, user);

        Ok(user_id)
    }

    /// Login with email + password. Returns JWT.
    pub fn login(&self, email: &str, password: &str) -> Result<LoginResponse, AuthError> {
        let user_id = self
            .users_by_email
            .get(email)
            .ok_or(AuthError::InvalidCredentials)?;
        let user = self
            .users
            .get(user_id)
            .ok_or(AuthError::InvalidCredentials)?;

        if !user.is_active {
            return Err(AuthError::InvalidCredentials);
        }

        verify_password(password, &user.password_hash)?;

        let now_secs = chrono::Utc::now().timestamp() as u64;
        let expires_at = now_secs + self.jwt_expiry_secs;

        let claims = JwtClaims {
            sub: user.user_id.value(),
            exp: expires_at,
            iat: now_secs,
            jti: Uuid::new_v4().to_string(),
        };

        let token = jsonwebtoken::encode(
            &Header::new(Algorithm::HS256),
            &claims,
            &EncodingKey::from_secret(&self.jwt_secret),
        )
        .map_err(|e| AuthError::Internal(format!("jwt encode: {e}")))?;

        Ok(LoginResponse {
            token,
            expires_at,
            user_id: user.user_id,
        })
    }

    /// Verify a JWT token. Returns claims.
    pub fn verify_jwt(&self, token: &str) -> Result<JwtClaims, AuthError> {
        let mut validation = Validation::new(Algorithm::HS256);
        validation.validate_exp = true;

        let token_data = jsonwebtoken::decode::<JwtClaims>(
            token,
            &DecodingKey::from_secret(&self.jwt_secret),
            &validation,
        )
        .map_err(|e| match e.kind() {
            jsonwebtoken::errors::ErrorKind::ExpiredSignature => AuthError::TokenExpired,
            _ => AuthError::InvalidToken(e.to_string()),
        })?;

        Ok(token_data.claims)
    }

    /// Enable TOTP 2FA. Returns the secret (base32) for QR code generation.
    pub fn enable_2fa(&mut self, user_id: UserId) -> Result<String, AuthError> {
        let user = self
            .users
            .get_mut(&user_id)
            .ok_or(AuthError::UserNotFound)?;

        // Generate 20-byte random secret for TOTP (standard RFC 4226 key length)
        let secret_bytes: [u8; 20] = rand::random();
        let secret = totp_rs::Secret::Raw(secret_bytes.to_vec());
        let secret_base32 = secret.to_encoded().to_string();
        user.totp_secret = Some(secret_base32.clone());
        user.updated_at = UnixMicros::now();

        Ok(secret_base32)
    }

    /// Verify TOTP code.
    pub fn verify_2fa(&self, user_id: UserId, code: &str) -> Result<bool, AuthError> {
        let user = self.users.get(&user_id).ok_or(AuthError::UserNotFound)?;
        let secret_b32 = user
            .totp_secret
            .as_ref()
            .ok_or(AuthError::TwoFactorNotEnabled)?;

        let totp = totp_rs::TOTP::new(
            totp_rs::Algorithm::SHA1,
            6,
            1,
            30,
            totp_rs::Secret::Encoded(secret_b32.clone())
                .to_bytes()
                .map_err(|e| AuthError::Internal(format!("totp secret decode: {e}")))?,
            Some("exg".to_string()),
            String::new(),
        )
        .map_err(|e| AuthError::Internal(format!("totp init: {e}")))?;

        let valid = totp
            .check_current(code)
            .map_err(|e| AuthError::Internal(format!("totp check: {e}")))?;

        if valid {
            Ok(true)
        } else {
            Err(AuthError::Invalid2faCode)
        }
    }

    /// Create an API key. Returns (key_id, secret) -- secret is only shown once.
    pub fn create_api_key(
        &mut self,
        user_id: UserId,
        label: &str,
        permissions: ApiPermissions,
    ) -> Result<(String, String), AuthError> {
        if !self.users.contains_key(&user_id) {
            return Err(AuthError::UserNotFound);
        }

        let key_id = Uuid::new_v4().to_string();

        // Generate 32-byte random secret, hex-encode for transport
        let secret_bytes: [u8; 32] = rand::random();
        let secret_hex = hex_encode(&secret_bytes);

        let api_key = ApiKey {
            key_id: key_id.clone(),
            user_id,
            secret_key: secret_hex.clone(),
            label: label.to_string(),
            permissions,
            is_active: true,
            created_at: UnixMicros::now(),
            ip_whitelist: Vec::new(),
        };

        self.api_keys.insert(key_id.clone(), api_key);

        Ok((key_id, secret_hex))
    }

    /// Verify API key signature.
    /// The client computes HMAC-SHA256(api_secret, message) and sends it as `signature`.
    /// `message` = timestamp + method + path + body.
    pub fn verify_api_key_signature(
        &self,
        key_id: &str,
        message: &str,
        signature: &str,
    ) -> Result<&ApiKey, AuthError> {
        let api_key = self.api_keys.get(key_id).ok_or(AuthError::ApiKeyNotFound)?;

        if !api_key.is_active {
            return Err(AuthError::ApiKeyRevoked);
        }

        // Recompute HMAC-SHA256(secret, message)
        let mut mac = HmacSha256::new_from_slice(api_key.secret_key.as_bytes())
            .map_err(|e| AuthError::Internal(format!("hmac init: {e}")))?;
        mac.update(message.as_bytes());
        let expected = hex_encode(&mac.finalize().into_bytes());

        if !constant_time_eq(expected.as_bytes(), signature.as_bytes()) {
            return Err(AuthError::InvalidSignature);
        }

        Ok(api_key)
    }

    /// Revoke an API key.
    pub fn revoke_api_key(&mut self, user_id: UserId, key_id: &str) -> Result<(), AuthError> {
        let api_key = self
            .api_keys
            .get_mut(key_id)
            .ok_or(AuthError::ApiKeyNotFound)?;

        if api_key.user_id != user_id {
            return Err(AuthError::PermissionDenied);
        }

        api_key.is_active = false;
        Ok(())
    }

    /// Create a sub-account.
    pub fn create_sub_account(
        &mut self,
        user_id: UserId,
        label: &str,
    ) -> Result<AccountId, AuthError> {
        if !self.users.contains_key(&user_id) {
            return Err(AuthError::UserNotFound);
        }

        let account_id = AccountId::new(self.next_account_id);
        self.next_account_id += 1;

        let sub = SubAccount {
            account_id,
            user_id,
            label: label.to_string(),
            is_active: true,
            created_at: UnixMicros::now(),
        };

        self.sub_accounts.insert(account_id, sub);
        Ok(account_id)
    }

    /// List sub-accounts for a user.
    pub fn list_sub_accounts(&self, user_id: UserId) -> Vec<&SubAccount> {
        self.sub_accounts
            .values()
            .filter(|sa| sa.user_id == user_id)
            .collect()
    }

    /// Get user by ID.
    pub fn get_user(&self, user_id: UserId) -> Option<&User> {
        self.users.get(&user_id)
    }

    /// Change password.
    pub fn change_password(
        &mut self,
        user_id: UserId,
        old_password: &str,
        new_password: &str,
    ) -> Result<(), AuthError> {
        validate_password(new_password)?;

        let user = self.users.get(&user_id).ok_or(AuthError::UserNotFound)?;
        verify_password(old_password, &user.password_hash)?;

        let new_hash = hash_password(new_password)?;
        let user = self
            .users
            .get_mut(&user_id)
            .ok_or(AuthError::UserNotFound)?;
        user.password_hash = new_hash;
        user.updated_at = UnixMicros::now();

        Ok(())
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────

fn validate_password(password: &str) -> Result<(), AuthError> {
    if password.len() < 8 {
        return Err(AuthError::WeakPassword(
            "password must be at least 8 characters".to_string(),
        ));
    }
    Ok(())
}

fn hash_password(password: &str) -> Result<String, AuthError> {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    let hash = argon2
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| AuthError::Internal(format!("hash: {e}")))?;
    Ok(hash.to_string())
}

fn verify_password(password: &str, hash: &str) -> Result<(), AuthError> {
    let parsed =
        PasswordHash::new(hash).map_err(|e| AuthError::Internal(format!("parse hash: {e}")))?;
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .map_err(|_| AuthError::InvalidCredentials)
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Constant-time comparison to prevent timing attacks.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

// ══════════════════════════════════════════════════════════════════════════
// Tests
// ══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    fn make_service() -> AuthService {
        AuthService::new(b"test-jwt-secret-key-32-bytes!!!!", 3600)
    }

    #[test]
    fn test_register_success() {
        let mut svc = make_service();
        let uid = svc.register("alice@example.com", "strongpass1").unwrap();
        assert_eq!(uid.value(), 1);
        let user = svc.get_user(uid).unwrap();
        assert_eq!(user.email, "alice@example.com");
    }

    #[test]
    fn test_register_duplicate_email() {
        let mut svc = make_service();
        svc.register("alice@example.com", "strongpass1").unwrap();
        let err = svc
            .register("alice@example.com", "strongpass2")
            .unwrap_err();
        assert!(matches!(err, AuthError::EmailAlreadyExists));
    }

    #[test]
    fn test_login_success() {
        let mut svc = make_service();
        let uid = svc.register("bob@example.com", "strongpass1").unwrap();
        let resp = svc.login("bob@example.com", "strongpass1").unwrap();
        assert_eq!(resp.user_id, uid);
        assert!(!resp.token.is_empty());
    }

    #[test]
    fn test_login_wrong_password() {
        let mut svc = make_service();
        svc.register("bob@example.com", "strongpass1").unwrap();
        let err = svc.login("bob@example.com", "wrongpassword").unwrap_err();
        assert!(matches!(err, AuthError::InvalidCredentials));
    }

    #[test]
    fn test_verify_valid_jwt() {
        let mut svc = make_service();
        let uid = svc.register("carol@example.com", "strongpass1").unwrap();
        let resp = svc.login("carol@example.com", "strongpass1").unwrap();
        let claims = svc.verify_jwt(&resp.token).unwrap();
        assert_eq!(claims.sub, uid.value());
    }

    #[test]
    fn test_verify_expired_jwt() {
        let svc = make_service();
        // Manually create a token with exp in the past
        let claims = JwtClaims {
            sub: 1,
            exp: 1_000_000, // far in the past
            iat: 999_000,
            jti: "test-jti".to_string(),
        };
        let token = jsonwebtoken::encode(
            &Header::new(Algorithm::HS256),
            &claims,
            &EncodingKey::from_secret(&svc.jwt_secret),
        )
        .unwrap();
        let err = svc.verify_jwt(&token).unwrap_err();
        assert!(matches!(err, AuthError::TokenExpired));
    }

    #[test]
    fn test_verify_tampered_jwt() {
        let mut svc = make_service();
        svc.register("eve@example.com", "strongpass1").unwrap();
        let resp = svc.login("eve@example.com", "strongpass1").unwrap();
        // Tamper with the token payload
        let tampered = format!("{}x", resp.token);
        let err = svc.verify_jwt(&tampered).unwrap_err();
        assert!(matches!(err, AuthError::InvalidToken(_)));
    }

    #[test]
    fn test_enable_2fa() {
        let mut svc = make_service();
        let uid = svc.register("frank@example.com", "strongpass1").unwrap();
        let secret = svc.enable_2fa(uid).unwrap();
        assert!(!secret.is_empty());
        let user = svc.get_user(uid).unwrap();
        assert_eq!(user.totp_secret.as_deref(), Some(secret.as_str()));
    }

    #[test]
    fn test_verify_2fa_correct_code() {
        let mut svc = make_service();
        let uid = svc.register("grace@example.com", "strongpass1").unwrap();
        let secret_b32 = svc.enable_2fa(uid).unwrap();

        // Generate the current TOTP code from the secret
        let totp = totp_rs::TOTP::new(
            totp_rs::Algorithm::SHA1,
            6,
            1,
            30,
            totp_rs::Secret::Encoded(secret_b32).to_bytes().unwrap(),
            Some("exg".to_string()),
            String::new(),
        )
        .unwrap();
        let code = totp.generate_current().unwrap();

        let result = svc.verify_2fa(uid, &code).unwrap();
        assert!(result);
    }

    #[test]
    fn test_create_api_key() {
        let mut svc = make_service();
        let uid = svc.register("henry@example.com", "strongpass1").unwrap();
        let perms = ApiPermissions {
            can_trade: true,
            can_withdraw: false,
            can_read: true,
        };
        let (key_id, secret) = svc.create_api_key(uid, "trading", perms).unwrap();
        assert!(!key_id.is_empty());
        assert!(!secret.is_empty());
        assert_eq!(secret.len(), 64); // 32 bytes hex-encoded
    }

    #[test]
    fn test_verify_api_key_signature() {
        let mut svc = make_service();
        let uid = svc.register("iris@example.com", "strongpass1").unwrap();
        let perms = ApiPermissions {
            can_trade: true,
            can_withdraw: false,
            can_read: true,
        };
        let (key_id, secret) = svc.create_api_key(uid, "trading", perms).unwrap();

        let message = "1234567890GET/api/v1/orders";

        // Client computes HMAC-SHA256(secret, message)
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(message.as_bytes());
        let signature = hex_encode(&mac.finalize().into_bytes());

        let api_key = svc
            .verify_api_key_signature(&key_id, message, &signature)
            .unwrap();
        assert_eq!(api_key.user_id, uid);
    }

    #[test]
    fn test_verify_wrong_signature() {
        let mut svc = make_service();
        let uid = svc.register("jack@example.com", "strongpass1").unwrap();
        let perms = ApiPermissions {
            can_trade: true,
            can_withdraw: false,
            can_read: true,
        };
        let (key_id, _secret) = svc.create_api_key(uid, "trading", perms).unwrap();

        let err = svc
            .verify_api_key_signature(&key_id, "some message", "badsignature")
            .unwrap_err();
        assert!(matches!(err, AuthError::InvalidSignature));
    }

    #[test]
    fn test_revoke_api_key() {
        let mut svc = make_service();
        let uid = svc.register("kate@example.com", "strongpass1").unwrap();
        let perms = ApiPermissions {
            can_trade: true,
            can_withdraw: false,
            can_read: true,
        };
        let (key_id, secret) = svc.create_api_key(uid, "trading", perms).unwrap();

        svc.revoke_api_key(uid, &key_id).unwrap();

        // Attempt to verify signature after revocation
        let message = "test";
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(message.as_bytes());
        let signature = hex_encode(&mac.finalize().into_bytes());

        let err = svc
            .verify_api_key_signature(&key_id, message, &signature)
            .unwrap_err();
        assert!(matches!(err, AuthError::ApiKeyRevoked));
    }

    #[test]
    fn test_create_sub_account() {
        let mut svc = make_service();
        let uid = svc.register("leo@example.com", "strongpass1").unwrap();
        let acc_id = svc.create_sub_account(uid, "trading-sub").unwrap();
        assert_eq!(acc_id.value(), 1);

        let subs = svc.list_sub_accounts(uid);
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0].label, "trading-sub");
    }

    #[test]
    fn test_change_password() {
        let mut svc = make_service();
        let uid = svc.register("mia@example.com", "oldpassword1").unwrap();

        svc.change_password(uid, "oldpassword1", "newpassword1")
            .unwrap();

        // Old password no longer works
        let err = svc.login("mia@example.com", "oldpassword1").unwrap_err();
        assert!(matches!(err, AuthError::InvalidCredentials));

        // New password works
        let resp = svc.login("mia@example.com", "newpassword1").unwrap();
        assert_eq!(resp.user_id, uid);
    }

    #[test]
    fn test_weak_password_rejected() {
        let mut svc = make_service();
        let err = svc.register("weak@example.com", "short").unwrap_err();
        assert!(matches!(err, AuthError::WeakPassword(_)));
    }
}
