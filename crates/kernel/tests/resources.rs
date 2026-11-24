use std::sync::Arc;
use timbre_kernel::{Ctx, Fallible, LogFilter, Logger, Registry, Resources, ThreadRuntime};

struct Settings(String);

fn command(ctx: &Ctx) -> Result<String, &'static str> {
    ctx.resource::<Settings>()
        .map(|settings| settings.0.clone())
        .ok_or("not configured")
}

#[test]
fn a_module_can_report_missing_settings_and_use_installed_settings() -> Fallible {
    let finish = |resources| {
        Registry::new().finish_with_resources(
            Logger::stdout(LogFilter::default()),
            Arc::new(ThreadRuntime::new()),
            resources,
        )
    };
    assert_eq!(
        command(&Ctx::root(&finish(Resources::new()))),
        Err("not configured")
    );
    let resources = Resources::new().with(Arc::new(Settings("music".into())))?;
    assert_eq!(command(&Ctx::root(&finish(resources))), Ok("music".into()));
    Ok(())
}

#[test]
fn duplicate_resources_fail_before_the_kernel_is_started() {
    let result = Resources::new()
        .with(Arc::new(Settings("first".into())))
        .and_then(|resources| resources.with(Arc::new(Settings("second".into()))));
    let Err(failure) = result else {
        panic!("the duplicate settings were accepted");
    };
    assert_eq!(failure.code().as_str(), "kernel.resource_taken");
}
