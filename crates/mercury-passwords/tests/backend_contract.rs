//! Runs the backend contract in `common/contract.rs` against every backend.

mod common;

use common::contract;

macro_rules! contract_suite {
    ($suite:ident, $make:expr) => {
        mod $suite {
            use super::*;

            #[test]
            fn empty_backend_lists_nothing() {
                let (_guard, backend) = $make;
                contract::empty_backend_lists_nothing(&backend);
            }

            #[test]
            fn create_read_update_delete() {
                let (_guard, backend) = $make;
                contract::create_read_update_delete(&backend);
            }

            #[test]
            fn timestamps_come_from_the_item() {
                let (_guard, backend) = $make;
                contract::timestamps_come_from_the_item(&backend);
            }

            #[test]
            fn two_usernames_on_one_origin() {
                let (_guard, backend) = $make;
                contract::two_usernames_on_one_origin(&backend);
            }

            #[test]
            fn one_username_on_two_origins() {
                let (_guard, backend) = $make;
                contract::one_username_on_two_origins(&backend);
            }

            #[test]
            fn form_and_http_auth_logins_are_separate() {
                let (_guard, backend) = $make;
                contract::form_and_http_auth_logins_are_separate(&backend);
            }

            #[test]
            fn hostile_keys_round_trip() {
                let (_guard, backend) = $make;
                contract::hostile_keys_round_trip(&backend);
            }

            #[test]
            fn passwords_round_trip_byte_for_byte() {
                let (_guard, backend) = $make;
                contract::passwords_round_trip_byte_for_byte(&backend);
            }

            #[test]
            fn remove_all_removes_every_login() {
                let (_guard, backend) = $make;
                let backend = contract::remove_all_removes_every_login(backend);
                assert!(backend.list().expect("list").is_empty());
            }
        }
    };
}

use mercury_passwords::{MemoryBackend, SecretBackend};

contract_suite!(memory, ((), MemoryBackend::new()));

// Every Keychain test runs on a temporary keychain file that the guard deletes.
#[cfg(target_os = "macos")]
contract_suite!(keychain, common::keychain::temp_keychain());
