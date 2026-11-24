mod commands;
mod lastuse;
mod provider;

use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use timbre_edge_config::Config;
use timbre_edge_sqlite::SharedDatabase;
use timbre_kernel::credential::{Credential, RejectReason};
use timbre_kernel::identity::{Identity, Permission};
use timbre_kernel::signals::{
    AuthFailed, DeviceIssued, DeviceRevoked, PermissionsChanged, UserAdded, UserRemoved,
};
use timbre_kernel::testing::Shared;
use timbre_kernel::{
    AuthOutcome, Ctx, Fallible, Format, LogFilter, Logger, Port, Registry, Resources, Secret,
    ThreadRuntime, block_on,
};

/// The address every test attempt comes from unless it says otherwise.
pub(crate) const CLIENT: IpAddr = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 10));

static NEXT: AtomicUsize = AtomicUsize::new(0);

/// A permission of another module, so an admin's full set can be told apart
/// from what `auth` itself declares.
const OTHER: Permission = Permission::new("other.listen");

fn other_module(registry: &mut Registry) -> Fallible {
    registry.permission(OTHER, "Listen to something")?;
    registry.on::<DeviceIssued>(|ctx, signal| {
        recorder(ctx).push(format!(
            "DeviceIssued device={} label={} user={} secret={}",
            signal.device,
            signal.label,
            signal.user.as_str(),
            signal.secret.expose()
        ));
        Ok(())
    });
    registry.on::<DeviceRevoked>(|ctx, signal| {
        recorder(ctx).push(format!(
            "DeviceRevoked device={} user={}",
            signal.device,
            signal.user.as_str()
        ));
        Ok(())
    });
    registry.on::<UserAdded>(|ctx, signal| {
        recorder(ctx).push(format!("UserAdded user={}", signal.user.as_str()));
        Ok(())
    });
    registry.on::<UserRemoved>(|ctx, signal| {
        recorder(ctx).push(format!("UserRemoved user={}", signal.user.as_str()));
        Ok(())
    });
    registry.on::<PermissionsChanged>(|ctx, signal| {
        recorder(ctx).push(format!(
            "PermissionsChanged user={} granted={:?} revoked={:?} admin={}",
            signal.user.as_str(),
            names(&signal.granted),
            signal.revoked,
            signal.admin
        ));
        Ok(())
    });
    registry.on::<AuthFailed>(|ctx, signal| {
        recorder(ctx).push(format!(
            "AuthFailed client={} user={:?} reason={}",
            signal.client,
            signal.user.as_ref().map(|user| user.as_str().to_owned()),
            signal.reason.as_str()
        ));
        Ok(())
    });
    registry.provide::<dyn Signals>(Arc::new(Recorded::default()))
}

fn names(permissions: &[Permission]) -> Vec<&str> {
    permissions.iter().map(|held| held.as_str()).collect()
}

/// Every signal the kernel and `auth` published, in order, as text: a test
/// that asserts what a secret must not leak can then search one string.
pub(crate) trait Signals: Port {
    fn lines(&self) -> &Mutex<Vec<String>>;
}

#[derive(Default)]
struct Recorded {
    lines: Mutex<Vec<String>>,
}

impl Port for Recorded {}

impl Signals for Recorded {
    fn lines(&self) -> &Mutex<Vec<String>> {
        &self.lines
    }
}

struct Pusher(Arc<dyn Signals>);

impl Pusher {
    fn push(&self, line: String) {
        self.0
            .lines()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(line);
    }
}

fn recorder(ctx: &Ctx) -> Pusher {
    Pusher(ctx.ports().get::<dyn Signals>().expect("the recorder port"))
}

pub(crate) struct Fixture {
    base: PathBuf,
    sink: Shared,
    output: Shared,
    ctx: Ctx,
}

impl Fixture {
    pub(crate) fn new() -> Self {
        Self::with_config("")
    }

    pub(crate) fn with_config(extra: &str) -> Self {
        Self::with_output(extra, None)
    }

    fn with_output(extra: &str, writer: Option<Box<dyn std::io::Write + Send>>) -> Self {
        let base = std::env::temp_dir().join(format!(
            "timbre-auth-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let config = base.join("config.toml");
        std::fs::write(&config, extra).unwrap();
        let sink = Shared::default();
        let output = Shared::default();
        let writer = writer.unwrap_or_else(|| Box::new(output.clone()));
        let resources = Resources::new()
            .with(Arc::new(Config::load(Some(&config)).unwrap()))
            .unwrap()
            .with(Arc::new(SharedDatabase::new(
                base.join("timbre.db"),
                |_, _| Ok(()),
            )))
            .unwrap()
            .with(Arc::new(timbre_kernel::command::Output::new(writer)))
            .unwrap();
        let runtime = Arc::new(ThreadRuntime::new());
        let kernel = Registry::new()
            .module("auth", crate::register)
            .unwrap()
            .module("other", other_module)
            .unwrap()
            .finish_with_resources(
                Logger::new(
                    LogFilter::parse("debug").unwrap(),
                    Format::Json,
                    Box::new(sink.clone()),
                    std::time::SystemTime::now,
                ),
                runtime.clone(),
                resources,
            );
        kernel.check_config().unwrap();
        Self {
            base,
            sink,
            output,
            ctx: Ctx::root(&kernel),
        }
    }

    pub(crate) fn ctx(&self) -> &Ctx {
        &self.ctx
    }

    /// Run a registered command exactly as the binary dispatches it.
    pub(crate) fn run(&self, line: &str) -> Fallible {
        let args: Vec<String> = line.split_whitespace().map(str::to_owned).collect();
        self.run_args(&args)
    }

    fn run_args(&self, args: &[String]) -> Fallible {
        let (handler, rest) = self
            .ctx
            .kernel()
            .find_command(args)
            .unwrap_or_else(|| panic!("no command matches {args:?}"));
        block_on(handler(self.ctx.clone(), rest))
    }

    /// Add a user and issue one device, returning its secret.
    pub(crate) fn device(&self, user: &str, label: &str) -> Secret<String> {
        self.run(&format!("user add {user}")).unwrap();
        self.issue(&format!("device issue {user} {label}"))
    }

    /// Run an issuing command and take the secret out of its signal, which
    /// is the only place in the process it exists after stdout.
    pub(crate) fn issue(&self, line: &str) -> Secret<String> {
        let before = self.signals().len();
        self.run(line).unwrap();
        let issued = self
            .signals()
            .into_iter()
            .skip(before)
            .find_map(|signal| Some(signal.split_once(" secret=")?.1.to_owned()))
            .expect("the issue published a secret");
        Secret::new(issued)
    }

    /// The id of the device issued last.
    pub(crate) fn last_device(&self) -> String {
        self.signals()
            .iter()
            .rev()
            .find_map(|signal| {
                let rest = signal.strip_prefix("DeviceIssued device=")?;
                Some(rest.split_whitespace().next()?.to_owned())
            })
            .expect("a device was issued")
    }

    pub(crate) fn authenticate(&self, secret: &Secret<String>) -> AuthOutcome {
        self.authenticate_from(secret, CLIENT)
    }

    pub(crate) fn authenticate_from(&self, secret: &Secret<String>, client: IpAddr) -> AuthOutcome {
        let credential = Credential::ApiKey(Secret::new(secret.expose().clone()));
        block_on(self.ctx.authenticate(credential, client))
    }

    /// The identity a secret authenticates as, or a test failure.
    pub(crate) fn identity(&self, secret: &Secret<String>) -> Identity {
        match self.authenticate(secret) {
            AuthOutcome::Recognized(identity) => identity,
            other => panic!("expected recognition, got {other:?}"),
        }
    }

    pub(crate) fn rejection(&self, secret: &Secret<String>) -> RejectReason {
        match self.authenticate(secret) {
            AuthOutcome::Rejected(reason) => reason,
            other => panic!("expected a rejection, got {other:?}"),
        }
    }

    pub(crate) fn log(&self) -> String {
        assert!(self.ctx.kernel().flush_log());
        self.sink.text()
    }

    pub(crate) fn signals(&self) -> Vec<String> {
        self.ctx
            .ports()
            .get::<dyn Signals>()
            .expect("the recorder port")
            .lines()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The digests taken and comparisons made by the provider so far.
    pub(crate) fn work(&self) -> (u64, u64) {
        crate::state(&self.ctx).unwrap().work().counts()
    }

    fn write(&self, query: &str) {
        let shared = crate::config::shared_db(&self.ctx).unwrap();
        shared
            .get(&self.ctx)
            .unwrap()
            .write(|sql| crate::store::exec(sql, query, []))
            .unwrap();
    }

    /// Everything the module stored, as text, so a test can assert that a
    /// value never reached the database.
    pub(crate) fn dump(&self, query: &str) -> Vec<String> {
        let shared = crate::config::shared_db(&self.ctx).unwrap();
        shared
            .get(&self.ctx)
            .unwrap()
            .read(|sql| {
                crate::store::all(sql, query, [], |row| {
                    let mut line = String::new();
                    for index in 0..row.as_ref().column_count() {
                        if index > 0 {
                            line.push('|');
                        }
                        line.push_str(&value(row, index));
                    }
                    Ok(line)
                })
            })
            .unwrap()
    }
}

fn value(row: &timbre_edge_sqlite::rusqlite::Row<'_>, index: usize) -> String {
    use timbre_edge_sqlite::rusqlite::types::ValueRef;
    match row.get_ref(index) {
        Ok(ValueRef::Null) | Err(_) => String::new(),
        Ok(ValueRef::Integer(value)) => value.to_string(),
        Ok(ValueRef::Real(value)) => value.to_string(),
        Ok(ValueRef::Text(bytes)) => String::from_utf8_lossy(bytes).into_owned(),
        Ok(ValueRef::Blob(bytes)) => timbre_edge_crypto::base32::encode(bytes),
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}
