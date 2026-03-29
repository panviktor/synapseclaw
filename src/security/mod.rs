//! Security subsystem — re-exported from fork_security crate.
//!
//! All security types, sandbox backends, and policy enforcement live in
//! `crates/fork_security/`. This module re-exports everything and adds
//! workspace_boundary (which depends on config/workspace types).

// Re-export everything from fork_security.
pub use fork_security::*;

// workspace_boundary stays here — it depends on crate::config::workspace::WorkspaceProfile.
pub mod workspace_boundary;

/// Redact sensitive values — re-exported from fork_core.
#[allow(unused_imports)]
pub use fork_core::domain::util::redact;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reexported_policy_and_pairing_types_are_usable() {
        let policy = SecurityPolicy::default();
        assert_eq!(policy.autonomy, AutonomyLevel::Supervised);

        let guard = PairingGuard::new(false, &[]);
        assert!(!guard.require_pairing());
    }

    #[test]
    fn reexported_secret_store_encrypt_decrypt_roundtrip() {
        let temp = tempfile::tempdir().unwrap();
        let store = SecretStore::new(temp.path(), false);

        let encrypted = store.encrypt("top-secret").unwrap();
        let decrypted = store.decrypt(&encrypted).unwrap();

        assert_eq!(decrypted, "top-secret");
    }

    #[test]
    fn redact_hides_most_of_value() {
        assert_eq!(redact("abcdefgh"), "abcd***");
        assert_eq!(redact("ab"), "***");
        assert_eq!(redact(""), "***");
        assert_eq!(redact("12345"), "1234***");
    }

    #[test]
    fn redact_handles_multibyte_utf8_without_panic() {
        let result = redact("密码是很长的秘密");
        assert!(result.ends_with("***"));
        assert!(result.is_char_boundary(result.len()));
    }
}
