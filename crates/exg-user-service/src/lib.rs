pub mod auth;
pub mod error;
pub mod user;
// pub mod repo; -- Task 3 will add this; do NOT add yet (would break build)

pub use auth::{
    AuthService, JwtClaims, LoginResponse,
    sign_jwt, verify_jwt, hash_password, verify_password,
};
pub use error::AuthError;
pub use user::{ApiKey, ApiPermissions, KycLevel, SubAccount, User};
