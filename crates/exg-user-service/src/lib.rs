pub mod auth;
pub mod error;
pub mod user;
pub mod repo;

pub use auth::{
    AuthService, JwtClaims, LoginResponse,
    sign_jwt, verify_jwt, hash_password, verify_password,
};
pub use error::AuthError;
pub use user::{ApiKey, ApiPermissions, KycLevel, SubAccount, User};
pub use repo::{
    UserRow, register_user, login_user, find_user_by_id,
    init_dummy_argon2_hash, init_dummy_argon2_hash_for_tests,
    DUMMY_ARGON2_HASH,
};
