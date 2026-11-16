//! The signals the kernel itself publishes.

use crate::bus::Signal;

/// Published once after every module is registered: the server is running.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Started;

impl Signal for Started {}
