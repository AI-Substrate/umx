//! ServiceManager contract. A factory builds a manager over a fresh, empty sandbox (a temp folder and a stub
//! launchctl or systemctl for the real adapters), so no clause touches the machine's real services.

use umx_core::ports::{ServiceError, ServiceManager, ServiceSpec, ServiceState};

pub const CLAUSES: &[&str] = &[
    "sm_c1_fresh_is_not_installed",
    "sm_c2_install_is_read_back_and_loaded",
    "sm_c3_installing_the_same_spec_again_is_an_upgrade",
    "sm_c4_another_program_or_home_is_a_conflict_and_changes_nothing",
    "sm_c5_uninstall_leaves_nothing_and_can_repeat",
    "sm_c6_an_unsafe_path_is_refused_and_changes_nothing",
    "sm_c7_restart_needs_an_installed_service",
];

pub fn spec() -> ServiceSpec {
    ServiceSpec {
        program: "/opt/umx bin/umx".to_string(),
        home: "/home/u/.umx".to_string(),
        log: "/home/u/.umx/daemon.log".to_string(),
        path: Some("/opt/umx bin:/usr/bin:/bin".to_string()),
        background: false,
    }
}

fn installed<P: ServiceManager>(port: &P) -> (Option<ServiceSpec>, bool) {
    match port.status().expect("status") {
        ServiceState::Installed { spec, loaded, .. } => (spec, loaded),
        ServiceState::NotInstalled => panic!("expected an installed service"),
    }
}

pub fn sm_c1_fresh_is_not_installed<P: ServiceManager>(factory: impl Fn() -> P) {
    let port = factory();
    assert_eq!(port.status().expect("status"), ServiceState::NotInstalled);
    assert!(port.adapter_id().ends_with(".service-manager"));
}

pub fn sm_c2_install_is_read_back_and_loaded<P: ServiceManager>(factory: impl Fn() -> P) {
    let port = factory();
    port.install(&spec()).expect("install");
    assert_eq!(installed(&port), (Some(spec()), true));
}

pub fn sm_c3_installing_the_same_spec_again_is_an_upgrade<P: ServiceManager>(
    factory: impl Fn() -> P,
) {
    let port = factory();
    port.install(&spec()).expect("install");
    port.install(&spec()).expect("install again");
    assert_eq!(installed(&port), (Some(spec()), true));
}

pub fn sm_c4_another_program_or_home_is_a_conflict_and_changes_nothing<P: ServiceManager>(
    factory: impl Fn() -> P,
) {
    let port = factory();
    port.install(&spec()).expect("install");
    let mut other_program = spec();
    other_program.program = "/elsewhere/umx".to_string();
    let mut other_home = spec();
    other_home.home = "/home/u/other".to_string();
    for other in [other_program, other_home] {
        match port.install(&other) {
            Err(ServiceError::Conflict { found }) => assert_eq!(found, Some(spec())),
            got => panic!("expected Conflict, got {got:?}"),
        }
        assert_eq!(installed(&port), (Some(spec()), true));
    }
}

pub fn sm_c5_uninstall_leaves_nothing_and_can_repeat<P: ServiceManager>(factory: impl Fn() -> P) {
    let port = factory();
    port.install(&spec()).expect("install");
    port.uninstall().expect("uninstall");
    assert_eq!(port.status().expect("status"), ServiceState::NotInstalled);
    port.uninstall().expect("uninstall again");
    // And it can be installed again afterwards, for another program too.
    let mut other = spec();
    other.program = "/elsewhere/umx".to_string();
    port.install(&other).expect("install after uninstall");
    assert_eq!(installed(&port), (Some(other), true));
}

pub fn sm_c6_an_unsafe_path_is_refused_and_changes_nothing<P: ServiceManager>(
    factory: impl Fn() -> P,
) {
    let port = factory();
    for bad in [
        "/a\"b/umx",
        "/a$HOME/umx",
        "/a%h/umx",
        "/a\nb/umx",
        "relative/umx",
    ] {
        let mut s = spec();
        s.program = bad.to_string();
        assert!(
            matches!(port.install(&s), Err(ServiceError::BadPath(_))),
            "{bad:?}"
        );
        let mut s = spec();
        s.home = bad.to_string();
        assert!(
            matches!(port.install(&s), Err(ServiceError::BadPath(_))),
            "{bad:?}"
        );
        let mut s = spec();
        s.path = Some(format!("/usr/bin:{bad}"));
        assert!(
            matches!(port.install(&s), Err(ServiceError::BadPath(_))),
            "PATH {bad:?}"
        );
    }
    assert_eq!(port.status().expect("status"), ServiceState::NotInstalled);
}

pub fn sm_c7_restart_needs_an_installed_service<P: ServiceManager>(factory: impl Fn() -> P) {
    let port = factory();
    assert!(port.restart().is_err(), "restart with nothing installed");
    port.install(&spec()).expect("install");
    port.restart().expect("restart");
    assert_eq!(installed(&port), (Some(spec()), true));
}

/// One `#[test]` per ServiceManager clause plus a test that the list is exactly `CLAUSES`. Usage:
/// `service_manager_contract!(factory; sm_c1_<what>(), ...)` with `factory: Fn() -> impl ServiceManager`, each clause
/// written as a call so umx-arch's r-fake-per-port token scan sees it.
#[macro_export]
macro_rules! service_manager_contract {
    ($factory:expr; $($clause:ident()),+ $(,)?) => {
        mod service_manager_contract {
            #[allow(unused_imports)]
            use super::*;
            $(
                #[test]
                fn $clause() {
                    $crate::service_manager::$clause($factory)
                }
            )+
            #[test]
            fn every_clause_is_listed() {
                $crate::assert_listed($crate::service_manager::CLAUSES, &[$(stringify!($clause)),+]);
            }
        }
    };
}
