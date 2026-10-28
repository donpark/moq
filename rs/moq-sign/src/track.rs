//! Signing producer and verifying subscriber for one track.

use std::fmt;
use std::sync::Arc;
use std::task::{Poll, ready};

use crate::error::Result;
use crate::group;
use crate::key::{PublicKey, SecretKey};
use crate::scope::Scope;

/// Producer for one signed track.
pub struct Producer {
	inner: moq_net::track::Producer,
	key: SecretKey,
	scope: Arc<Scope>,
}

impl Producer {
	pub(crate) fn new(inner: moq_net::track::Producer, key: SecretKey, scope: Arc<Scope>) -> Self {
		Self { inner, key, scope }
	}

	/// The track name.
	pub fn name(&self) -> &str {
		self.inner.name()
	}

	/// Start a group at the next sequence.
	///
	/// # Errors
	///
	/// A net error if the track is closed.
	pub fn append_group(&self) -> Result<group::Producer> {
		let inner = self.inner.append_group()?;
		Ok(self.group(inner))
	}

	/// Start a group at an explicit sequence.
	///
	/// # Errors
	///
	/// A net error if the track is closed or already holds the sequence.
	pub fn create_group(&self, sequence: u64) -> Result<group::Producer> {
		let inner = self.inner.create_group(moq_net::group::Info { sequence })?;
		Ok(self.group(inner))
	}

	fn group(&self, inner: moq_net::group::Producer) -> group::Producer {
		group::Producer::new(inner, self.key.clone(), Arc::clone(&self.scope))
	}

	/// Finish the track after the last group.
	///
	/// # Errors
	///
	/// A net error if the track is already closed.
	pub fn finish(self) -> Result<()> {
		self.inner.finish()?;
		Ok(())
	}

	/// Abort the track with `err`, consuming the handle.
	///
	/// # Errors
	///
	/// A net error if the track is already closed.
	pub fn abort(self, err: moq_net::Error) -> Result<()> {
		self.inner.abort(err)?;
		Ok(())
	}
}

impl fmt::Debug for Producer {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.debug_struct("track::Producer").field("name", &self.name()).finish()
	}
}

/// The pollable state of a [`broadcast::Consumer::subscribe`](crate::broadcast::Consumer::subscribe);
/// awaited via the [`kio::Pending`] wrapper, whose `DerefMut` exposes [`Self::update`].
pub struct Subscribing {
	inner: kio::Pending<moq_net::track::Subscribing>,
	key: PublicKey,
	// Bound to the name subscribed to, never to the name a relay hands back.
	scope: Arc<Scope>,
}

impl Subscribing {
	pub(crate) fn new(inner: kio::Pending<moq_net::track::Subscribing>, key: PublicKey, scope: Arc<Scope>) -> Self {
		Self { inner, key, scope }
	}

	/// Poll until the track is found, yielding a verifying [`Consumer`].
	///
	/// # Errors
	///
	/// A net error if the track is aborted or not found.
	pub fn poll_ok(&self, waiter: &kio::Waiter) -> Poll<Result<Consumer>> {
		let inner = ready!(self.inner.poll_ok(waiter))?;
		Poll::Ready(Ok(Consumer {
			inner,
			key: self.key,
			scope: Arc::clone(&self.scope),
		}))
	}

	/// Change the subscription preferences before (or after) it resolves.
	///
	/// # Errors
	///
	/// [`moq_net::Error::Closed`] if the track already ended.
	pub fn update(&mut self, subscription: moq_net::track::Subscription) -> Result<()> {
		Ok(self.inner.update(subscription)?)
	}
}

impl kio::Task for Subscribing {
	type Output = Result<Consumer>;

	fn poll(&mut self, waiter: &kio::Waiter) -> Poll<Self::Output> {
		self.poll_ok(waiter)
	}
}

/// Subscriber for one signed track.
///
/// Each group verifies on its own: a forged frame ends only the group that carried it.
pub struct Consumer {
	inner: moq_net::track::Subscriber,
	key: PublicKey,
	scope: Arc<Scope>,
}

impl Consumer {
	/// The track name.
	pub fn name(&self) -> &str {
		self.inner.name()
	}

	/// Poll for the next group in arrival order.
	///
	/// # Errors
	///
	/// A net error if the track fails.
	pub fn poll_recv_group(&mut self, waiter: &kio::Waiter) -> Poll<Result<Option<group::Consumer>>> {
		let Some(inner) = ready!(self.inner.poll_recv_group(waiter)?) else {
			return Poll::Ready(Ok(None));
		};
		Poll::Ready(Ok(Some(group::Consumer::new(inner, self.key, Arc::clone(&self.scope)))))
	}

	/// Receive the next group in arrival order.
	///
	/// # Errors
	///
	/// Same as [`Self::poll_recv_group`].
	pub async fn recv_group(&mut self) -> Result<Option<group::Consumer>> {
		kio::wait(|waiter| self.poll_recv_group(waiter)).await
	}
}

impl fmt::Debug for Consumer {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.debug_struct("track::Consumer").field("name", &self.name()).finish()
	}
}
