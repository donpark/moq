//! Control-message byte counters across the shared-runtime and io_uring listener boundary.
#![cfg(feature = "_quic")]

use moq_net::stats::{Control, Registry};
use moq_relay::{Config, Relay};
use moq_tokio::moq_net;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(10);

/// Whether the relay has both sent and received announce control bytes.
fn announced(registry: &Registry) -> bool {
	registry
		.snapshot()
		.control()
		.iter()
		.any(|(_, class, bytes)| *class == Control::Announce && bytes.tx > 0 && bytes.rx > 0)
}

async fn control_counters(io_uring: bool) {
	let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
	let dir = tempfile::tempdir().unwrap();
	let key = rcgen::KeyPair::generate().unwrap();
	let cert = rcgen::CertificateParams::new(vec!["localhost".into()])
		.unwrap()
		.self_signed(&key)
		.unwrap();
	let cert_path = dir.path().join("cert.pem");
	let key_path = dir.path().join("key.pem");
	std::fs::write(&cert_path, cert.pem()).unwrap();
	std::fs::write(&key_path, key.serialize_pem()).unwrap();
	let mut config = Config::default();
	config.listen.bind = Some("127.0.0.1:0".parse().unwrap());
	config.listen.tls.cert = vec![cert_path];
	config.listen.tls.key = vec![key_path];
	config.auth.public = vec![moq_auth::Pattern::all()];
	config.stats.enabled = true;
	config.internal.listen = Some("127.0.0.1:0".parse().unwrap());
	config.stats.interval = 3600;
	config.drain_timeout = Duration::ZERO;
	if io_uring {
		config.runtime.workers = Some(1);
		config.runtime.pin = false;
		config.runtime.io_uring = true;
	}
	let relay = Relay::load(config).await.unwrap().with_signals(false);
	let addr = relay.addr().unwrap();
	let metrics = format!("http://{}/metrics", relay.internal().addr().unwrap());
	let registry = relay.cluster().stats.clone();
	let trigger = relay.shutdown_trigger().clone();
	let running = tokio::spawn(relay.run());
	for scheme in ["moql", "https"] {
		let mut client = moq_tokio::connect::Config::default();
		client.tls.insecure = Some(true);
		client.bind = Some("127.0.0.1:0".parse().unwrap());
		let client = client.init(Default::default()).unwrap();
		let url: url::Url = format!("{scheme}://{addr}/control").parse().unwrap();
		let connection = tokio::time::timeout(TIMEOUT, client.with_reconnect(false).connect(url).established())
			.await
			.unwrap()
			.unwrap();
		tokio::time::timeout(TIMEOUT, async {
			while !announced(&registry) {
				tokio::task::yield_now().await;
			}
		})
		.await
		.expect("the session exchanges announce control messages");
		let body = reqwest::get(&metrics)
			.await
			.unwrap()
			.error_for_status()
			.unwrap()
			.text()
			.await
			.unwrap();
		for direction in ["tx", "rx"] {
			let metric = format!("moq_relay_control_{direction}_bytes_total");
			assert!(body.contains(&format!("# TYPE {metric} counter")), "{body}");
			let prefix = format!("{metric}{{tier=\"\",class=\"announce\"}} ");
			let total: u64 = body
				.lines()
				.find_map(|line| line.strip_prefix(&prefix))
				.expect("control metric")
				.parse()
				.unwrap();
			assert!(total > 0, "{scheme}: {body}");
		}
		drop(connection);
	}
	trigger.start();
	tokio::time::timeout(TIMEOUT, running).await.unwrap().unwrap().unwrap();
}

#[tokio::test]
async fn native_sessions_count_control_bytes() {
	control_counters(false).await;
}

#[cfg(all(target_os = "linux", feature = "_uring"))]
#[tokio::test]
async fn uring_sessions_count_control_bytes() {
	match moq_uring::Worker::new(Default::default()) {
		Ok(_) => (),
		Err(moq_uring::Error::Unsupported(reason)) if std::env::var_os("MOQ_STRICT").is_none() => {
			eprintln!("skipping io_uring control stats test: {reason}");
			return;
		}
		Err(error) => panic!("io_uring worker setup failed: {error}"),
	}
	control_counters(true).await;
}
