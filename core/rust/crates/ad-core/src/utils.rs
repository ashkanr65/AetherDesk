//! Utility functions for AetherDesk core

use uuid::Uuid;

/// Generate a new unique identifier
pub fn generate_id() -> String {
    Uuid::new_v4().to_string()
}

/// Sanitize a string for logging (remove potential secrets)
pub fn sanitize_for_logging(input: &str) -> String {
    // Simple sanitization - in production, this should be more sophisticated
    input
        .replace("password", "***")
        .replace("secret", "***")
        .replace("token", "***")
        .replace("key", "***")
}
