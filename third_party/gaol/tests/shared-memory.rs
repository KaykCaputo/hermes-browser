// Any copyright is dedicated to the Public Domain.
// http://creativecommons.org/publicdomain/zero/1.0/

extern crate gaol;
extern crate libc;

use gaol::profile::Profile;
use gaol::sandbox::{ChildSandbox, ChildSandboxMethods, Command, Sandbox, SandboxMethods};
use std::env;
use std::ffi::CString;

const CHILD_ARGUMENT: &'static str = "shared_memory_test";
const SHARED_MEMORY_SIZE: libc::off_t = 350_704;

fn profile() -> Profile {
    Profile::new(Vec::new()).unwrap()
}

#[cfg(target_os="linux")]
fn shared_memory_test() {
    ChildSandbox::new(profile()).activate().unwrap();

    let name = CString::new("gaol-shared-memory-test").unwrap();
    let descriptor = unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC) };
    assert!(descriptor >= 0);
    assert_eq!(unsafe { libc::ftruncate(descriptor, SHARED_MEMORY_SIZE) }, 0);
    assert_eq!(unsafe { libc::close(descriptor) }, 0);
}

#[cfg(target_os="linux")]
pub fn main() {
    if env::args().skip(1).next().as_ref().map(String::as_str) == Some(CHILD_ARGUMENT) {
        return shared_memory_test()
    }

    let status = Sandbox::new(profile())
        .start(Command::me().unwrap().arg(CHILD_ARGUMENT))
        .unwrap()
        .wait()
        .unwrap();
    assert!(status.success());
}

#[cfg(not(target_os="linux"))]
fn main() {}
