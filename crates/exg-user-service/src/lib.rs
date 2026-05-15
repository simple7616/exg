pub mod auth;
pub mod error;
pub mod repo;
pub mod user;

pub use auth::{
    AuthService, JwtClaims, LoginResponse, hash_password, sign_jwt, verify_jwt, verify_password,
};
pub use error::AuthError;
pub use repo::{
    DUMMY_ARGON2_HASH, UserRow, find_user_by_id, init_dummy_argon2_hash,
    init_dummy_argon2_hash_for_tests, login_user, register_user,
};
pub use user::{ApiKey, ApiPermissions, KycLevel, SubAccount, User};
