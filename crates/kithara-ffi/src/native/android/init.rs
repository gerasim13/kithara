use jni::{Env, objects::JObject, sys::jint};
use rustls_platform_verifier::android as rustls_android;
use thiserror::Error;
use tracing_subscriber::{filter::LevelFilter, prelude::*};

#[derive(Debug, Error)]
pub(super) enum InitError {
    #[error(transparent)]
    Jni(#[from] jni::errors::Error),

    #[error("failed to initialize the rustls platform verifier: {details}")]
    RustlsVerifier { details: String },
}

pub(super) fn run(
    env: &mut Env<'_>,
    context: JObject<'_>,
    log_level: jint,
) -> Result<(), InitError> {
    install_logging(log_level);

    let vm = env.get_java_vm()?;
    let context_ref = env.new_global_ref(&context)?;
    kithara_android::initialize(&vm, context_ref);

    rustls_android::init_with_env(env, context).map_err(|err| InitError::RustlsVerifier {
        details: err.to_string(),
    })
}

fn install_logging(log_level: jint) {
    let Ok(layer) = tracing_android::layer("kithara") else {
        return;
    };
    let _ = tracing_subscriber::registry()
        .with(layer.with_filter(level_filter(log_level)))
        .try_init();
}

fn level_filter(ordinal: jint) -> LevelFilter {
    const LOG_LEVEL_INFO: jint = 2;
    const LOG_LEVEL_WARN: jint = 3;
    const LOG_LEVEL_ERROR: jint = 4;

    match ordinal {
        0 => LevelFilter::TRACE,
        1 => LevelFilter::DEBUG,
        LOG_LEVEL_INFO => LevelFilter::INFO,
        LOG_LEVEL_WARN => LevelFilter::WARN,
        LOG_LEVEL_ERROR => LevelFilter::ERROR,
        _ => LevelFilter::OFF,
    }
}
