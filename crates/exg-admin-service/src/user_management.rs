use std::collections::HashSet;

use exg_common::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserSummary {
    pub user_id: UserId,
    pub email: String,
    pub kyc_level: String,
    pub is_active: bool,
    pub created_at: UnixMicros,
}

pub struct UserManagement {
    /// Frozen users.
    frozen_users: HashSet<UserId>,
}

impl UserManagement {
    pub fn new() -> Self {
        Self {
            frozen_users: HashSet::new(),
        }
    }

    /// Freeze a user. Returns `true` if newly frozen.
    pub fn freeze_user(&mut self, user_id: UserId) -> bool {
        self.frozen_users.insert(user_id)
    }

    /// Unfreeze a user. Returns `true` if the user was previously frozen.
    pub fn unfreeze_user(&mut self, user_id: UserId) -> bool {
        self.frozen_users.remove(&user_id)
    }

    /// Check whether a user is frozen.
    pub fn is_frozen(&self, user_id: UserId) -> bool {
        self.frozen_users.contains(&user_id)
    }

    /// List all frozen user IDs.
    pub fn frozen_users(&self) -> Vec<UserId> {
        self.frozen_users.iter().copied().collect()
    }
}

impl Default for UserManagement {
    fn default() -> Self {
        Self::new()
    }
}
