//! GATT controller link: drives the Java BleController and polls its state.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use jni::objects::{GlobalRef, JClass, JString, JValue};

use crate::input::{ButtonStates, ControllerLink};
use crate::insets::with_activity;

const CLASS_NAME: &str = "com.kingsofalchemy.gblab.BleController";

const STATE_CONNECTED: i32 = 3;
const STATE_FAILED: i32 = 4;

const RETRY_EVERY: Duration = Duration::from_secs(3);

static CLASS: OnceLock<Option<GlobalRef>> = OnceLock::new();

fn class() -> Option<&'static GlobalRef> {
    CLASS
        .get_or_init(|| {
            with_activity(|env, activity| {
                let loader = env
                    .call_method(activity, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])?
                    .l()?;
                let name = env.new_string(CLASS_NAME)?;
                let class = env
                    .call_method(
                        &loader,
                        "loadClass",
                        "(Ljava/lang/String;)Ljava/lang/Class;",
                        &[JValue::Object(&name)],
                    )?
                    .l()?;
                env.new_global_ref(class)
            })
        })
        .as_ref()
}

fn static_int(field: &str) -> Option<i32> {
    let class = class()?;
    with_activity(|env, _| {
        let k = JClass::from(env.new_local_ref(class.as_obj())?);
        env.get_static_field(&k, field, "I")?.i()
    })
}

fn static_string(field: &str) -> Option<String> {
    let class = class()?;
    with_activity(|env, _| {
        let k = JClass::from(env.new_local_ref(class.as_obj())?);
        let obj = env.get_static_field(&k, field, "Ljava/lang/String;")?.l()?;
        let s: String = env.get_string(&JString::from(obj))?.into();
        Ok(s)
    })
}

fn call_start() {
    if let Some(class) = class() {
        with_activity(|env, activity| {
            let k = JClass::from(env.new_local_ref(class.as_obj())?);
            env.call_static_method(
                &k,
                "start",
                "(Landroid/app/Activity;)V",
                &[JValue::Object(activity)],
            )?
            .v()
        });
    }
}

fn call_stop() {
    if let Some(class) = class() {
        with_activity(|env, _| {
            let k = JClass::from(env.new_local_ref(class.as_obj())?);
            env.call_static_method(&k, "stop", "()V", &[])?.v()
        });
    }
}

pub struct BleLink {
    enabled: bool,
    next_retry: Instant,
}

impl BleLink {
    pub fn new() -> Self {
        BleLink { enabled: false, next_retry: Instant::now() }
    }
}

impl ControllerLink for BleLink {
    fn poll(&mut self) -> Option<ButtonStates> {
        if !self.enabled {
            return None;
        }
        let state = static_int("state").unwrap_or(STATE_FAILED);
        if state == STATE_CONNECTED {
            let bits = static_int("buttons").unwrap_or(0) as u8;
            let mut out = [false; 8];
            for (i, o) in out.iter_mut().enumerate() {
                *o = bits & (1 << i) != 0;
            }
            return Some(out);
        }
        // Not connected: keep (re)trying while enabled.
        let now = Instant::now();
        if now >= self.next_retry {
            self.next_retry = now + RETRY_EVERY;
            call_start();
        }
        None
    }

    fn status(&self) -> String {
        if !self.enabled {
            return "pad off".into();
        }
        match static_int("state") {
            Some(STATE_CONNECTED) => "pad connected".into(),
            Some(2) => "pad: connecting...".into(),
            Some(1) => "pad: allow bluetooth".into(),
            Some(STATE_FAILED) => {
                let detail = static_string("detail").unwrap_or_default();
                format!("pad: {detail}")
            }
            _ => "pad: searching...".into(),
        }
    }

    fn set_enabled(&mut self, on: bool) {
        if self.enabled == on {
            return;
        }
        self.enabled = on;
        if on {
            self.next_retry = Instant::now();
        } else {
            call_stop();
        }
    }

    fn enabled(&self) -> bool {
        self.enabled
    }
}
