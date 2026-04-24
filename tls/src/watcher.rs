// SPDX-License-Identifier: MIT
// Copyright (c) 2024 Shane Utt

//! Filesystem watcher for TLS certificate hot-reload.
//!
//! [`CertWatcher`] monitors cert and key files using the [`notify`]
//! crate, debounces events, and calls [`ReloadableCertResolver::reload`]
//! on detected changes.
//!
//! [`CertWatcher`]: crate::watcher::CertWatcher
//! [`notify`]: notify
//! [`ReloadableCertResolver::reload`]: crate::reload::ReloadableCertResolver::reload

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use arc_swap::ArcSwap;
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use rustls::sign::CertifiedKey;
use tokio::sync::mpsc;

use crate::{CertKeyPair, setup::loader};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Debounce window for filesystem events.
const DEBOUNCE_MS: u64 = 500;

// ---------------------------------------------------------------------------
// CertWatcher
// ---------------------------------------------------------------------------

/// Watches cert and key files for changes, reloading on modification.
///
/// Spawns as a tokio background task. Debounces events by
/// [`DEBOUNCE_MS`] to handle atomic rename patterns (Kubernetes
/// secret updates, certbot, etc.).
///
/// ```ignore
/// let handle = CertWatcher::spawn(resolver_arc, pair, shutdown_rx);
/// ```
///
/// [`DEBOUNCE_MS`]: DEBOUNCE_MS
pub struct CertWatcher;

impl CertWatcher {
    /// Spawn a background thread that watches cert/key paths and
    /// reloads the resolver on changes.
    ///
    /// Creates its own single-threaded tokio runtime so it works
    /// regardless of whether a tokio runtime is currently active
    /// (e.g. during Pingora service registration before the
    /// server starts).
    ///
    /// The thread runs for the lifetime of the process. Use
    /// `shutdown` to request early termination: send `true` to
    /// stop the watcher, or drop the sender to keep it running
    /// indefinitely.
    ///
    /// # Panics
    ///
    /// Panics if the tokio runtime cannot be created.
    #[allow(clippy::expect_used, reason = "fatal if tokio runtime cannot start")]
    pub fn spawn(
        current: Arc<ArcSwap<CertifiedKey>>,
        pair: CertKeyPair,
        shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("cert watcher tokio runtime");
            rt.block_on(watch_loop(current, pair, shutdown));
        })
    }
}

/// Core watch loop: sets up the notify watcher, debounces events,
/// and reloads certificates.
#[allow(
    clippy::cognitive_complexity,
    clippy::too_many_lines,
    reason = "event loop with tokio::select"
)]
async fn watch_loop(
    current: Arc<ArcSwap<CertifiedKey>>,
    pair: CertKeyPair,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    let (tx, mut rx) = mpsc::channel::<()>(16);

    let cert_dir = parent_dir(&pair.cert_path);
    let key_dir = parent_dir(&pair.key_path);

    let _watcher = match setup_watcher(tx, &cert_dir, &key_dir) {
        Ok(w) => w,
        Err(e) => {
            tracing::warn!(error = %e, "failed to start certificate file watcher");
            return;
        },
    };

    tracing::info!(
        cert_path = %pair.cert_path,
        key_path = %pair.key_path,
        "certificate file watcher started"
    );

    loop {
        tokio::select! {
            Some(()) = rx.recv() => {
                tracing::debug!("filesystem change detected, debouncing");
                drain_and_debounce(&mut rx).await;
                reload_cert(&current, &pair);
            }
            result = shutdown.changed() => {
                if result.is_ok() && *shutdown.borrow() {
                    tracing::info!("certificate file watcher shutting down");
                    return;
                }
            }
        }
    }
}

/// Set up a [`RecommendedWatcher`] that sends to the given channel
/// on relevant filesystem events.
///
/// [`RecommendedWatcher`]: notify::RecommendedWatcher
fn setup_watcher(tx: mpsc::Sender<()>, cert_dir: &Path, key_dir: &Path) -> Result<RecommendedWatcher, notify::Error> {
    let mut watcher = notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| match res {
        Ok(event) if is_relevant_event(event.kind) => {
            let _sent = tx.blocking_send(());
        },
        Err(e) => {
            tracing::warn!(error = %e, "file watcher error");
        },
        _ => {},
    })?;

    watcher.watch(cert_dir, RecursiveMode::NonRecursive)?;
    if cert_dir != key_dir {
        watcher.watch(key_dir, RecursiveMode::NonRecursive)?;
    }

    Ok(watcher)
}

/// Drain pending events and sleep for the debounce window.
async fn drain_and_debounce(rx: &mut mpsc::Receiver<()>) {
    tokio::time::sleep(Duration::from_millis(DEBOUNCE_MS)).await;
    while rx.try_recv().is_ok() {}
}

/// Attempt to reload the certificate, logging success or failure.
fn reload_cert(current: &Arc<ArcSwap<CertifiedKey>>, pair: &CertKeyPair) {
    match loader::load_certified_key(pair) {
        Ok(certified) => {
            current.store(Arc::new(certified));
            tracing::info!(
                cert_path = %pair.cert_path,
                "TLS certificate hot-reloaded successfully"
            );
        },
        Err(e) => {
            tracing::warn!(
                cert_path = %pair.cert_path,
                error = %e,
                "TLS certificate reload failed, keeping previous certificate"
            );
        },
    }
}

// ---------------------------------------------------------------------------
// Utility Functions
// ---------------------------------------------------------------------------

/// Whether a notify event kind is relevant for cert reload.
fn is_relevant_event(kind: EventKind) -> bool {
    matches!(kind, EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_))
}

/// Extract the parent directory of a path, defaulting to `.`.
fn parent_dir(path: &str) -> PathBuf {
    Path::new(path).parent().unwrap_or_else(|| Path::new(".")).to_path_buf()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::used_underscore_binding,
    reason = "tests"
)]
mod tests {
    use super::*;

    #[test]
    fn parent_dir_extracts_directory() {
        let dir = parent_dir("/etc/ssl/certs/server.pem");
        assert_eq!(dir, PathBuf::from("/etc/ssl/certs"), "should extract parent");
    }

    #[test]
    fn parent_dir_root_file() {
        let dir = parent_dir("/cert.pem");
        assert_eq!(dir, PathBuf::from("/"), "root file parent should be /");
    }

    #[test]
    fn is_relevant_event_create() {
        assert!(
            is_relevant_event(EventKind::Create(notify::event::CreateKind::File)),
            "Create events should be relevant"
        );
    }

    #[test]
    fn is_relevant_event_modify() {
        assert!(
            is_relevant_event(EventKind::Modify(notify::event::ModifyKind::Data(
                notify::event::DataChange::Content
            ))),
            "Modify events should be relevant"
        );
    }

    #[test]
    fn is_relevant_event_access_is_not_relevant() {
        assert!(
            !is_relevant_event(EventKind::Access(notify::event::AccessKind::Read)),
            "Access events should not be relevant"
        );
    }

    #[test]
    fn spawn_watcher_shuts_down_on_signal() {
        let certs = gen_test_certs();
        let pair = CertKeyPair {
            cert_path: certs.cert_path.to_str().expect("cert path").to_owned(),
            default: false,
            key_path: certs.key_path.to_str().expect("key path").to_owned(),
            server_names: Vec::new(),
        };
        let certified = loader::load_certified_key(&pair).expect("load cert");
        let current = Arc::new(ArcSwap::from_pointee(certified));

        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let handle = CertWatcher::spawn(current, pair, shutdown_rx);

        std::thread::sleep(Duration::from_millis(50));
        let _sent = shutdown_tx.send(true);

        let result = handle.join();
        assert!(result.is_ok(), "watcher thread should shut down cleanly");
    }

    #[test]
    fn watcher_reloads_on_file_change() {
        let certs = gen_test_certs();
        let pair = CertKeyPair {
            cert_path: certs.cert_path.to_str().expect("cert path").to_owned(),
            default: false,
            key_path: certs.key_path.to_str().expect("key path").to_owned(),
            server_names: Vec::new(),
        };
        let certified = loader::load_certified_key(&pair).expect("load cert");
        let before_der = certified.cert[0].as_ref().to_vec();
        let current = Arc::new(ArcSwap::from_pointee(certified));

        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let _handle = CertWatcher::spawn(Arc::clone(&current), pair.clone(), shutdown_rx);

        std::thread::sleep(Duration::from_millis(100));

        let new_certs = gen_test_certs_in(&certs._temp_dir);
        drop(new_certs);

        std::thread::sleep(Duration::from_millis(1500));

        let after_der = current.load_full().cert[0].as_ref().to_vec();

        let _sent = shutdown_tx.send(true);

        assert_ne!(
            before_der, after_der,
            "certificate should change after file modification"
        );
    }

    // ---------------------------------------------------------------------------
    // Test Utilities
    // ---------------------------------------------------------------------------

    /// Generated test certificate bundle with temp dir lifetime.
    struct TestCerts {
        /// Path to the server certificate PEM.
        cert_path: PathBuf,

        /// Path to the server private key PEM.
        key_path: PathBuf,

        /// Temp directory holding the cert files.
        _temp_dir: tempfile::TempDir,
    }

    /// Generate a self-signed CA and server certificate for testing.
    fn gen_test_certs() -> TestCerts {
        use rcgen::{CertificateParams, DnType, IsCa, KeyPair};

        let ca_key = KeyPair::generate().expect("CA key generation");
        let mut ca_params = CertificateParams::new(Vec::<String>::new()).expect("CA params");
        ca_params.is_ca = IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        ca_params.distinguished_name.push(DnType::CommonName, "Test CA");
        let ca_cert = ca_params.self_signed(&ca_key).expect("CA self-sign");

        let server_key = KeyPair::generate().expect("server key generation");
        let mut server_params = CertificateParams::new(vec!["localhost".to_owned()]).expect("server params");
        server_params.distinguished_name.push(DnType::CommonName, "localhost");
        let server_cert = server_params
            .signed_by(&server_key, &ca_cert, &ca_key)
            .expect("server cert sign");

        let temp_dir = tempfile::TempDir::new().expect("tempdir");
        let cert_path = temp_dir.path().join("server.pem");
        let key_path = temp_dir.path().join("server-key.pem");

        std::fs::write(&cert_path, server_cert.pem()).expect("write cert PEM");
        std::fs::write(&key_path, server_key.serialize_pem()).expect("write key PEM");

        TestCerts {
            cert_path,
            key_path,
            _temp_dir: temp_dir,
        }
    }

    /// Generate new certs in an existing temp dir, overwriting the files.
    fn gen_test_certs_in(dir: &tempfile::TempDir) -> TestCerts {
        use rcgen::{CertificateParams, DnType, IsCa, KeyPair};

        let ca_key = KeyPair::generate().expect("CA key generation");
        let mut ca_params = CertificateParams::new(Vec::<String>::new()).expect("CA params");
        ca_params.is_ca = IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        ca_params.distinguished_name.push(DnType::CommonName, "Test CA 2");
        let ca_cert = ca_params.self_signed(&ca_key).expect("CA self-sign");

        let server_key = KeyPair::generate().expect("server key generation");
        let mut server_params = CertificateParams::new(vec!["localhost".to_owned()]).expect("server params");
        server_params.distinguished_name.push(DnType::CommonName, "localhost");
        let server_cert = server_params
            .signed_by(&server_key, &ca_cert, &ca_key)
            .expect("server cert sign");

        let cert_path = dir.path().join("server.pem");
        let key_path = dir.path().join("server-key.pem");

        std::fs::write(&cert_path, server_cert.pem()).expect("write cert PEM");
        std::fs::write(&key_path, server_key.serialize_pem()).expect("write key PEM");

        TestCerts {
            cert_path,
            key_path,
            _temp_dir: tempfile::TempDir::new().expect("dummy dir"),
        }
    }
}
