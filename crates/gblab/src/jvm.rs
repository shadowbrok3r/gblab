//! JNI plumbing: VM attachment, Application context, and Activity handles.

use std::ffi::c_void;
use std::sync::Mutex;

use jni::JavaVM;
use jni::objects::{GlobalRef, JObject};

// ndk-context's context is the Application (android-activity 0.6.1), so the
// Activity jobject is captured separately in android_main.
static ACTIVITY: Mutex<Option<GlobalRef>> = Mutex::new(None);

/// Stores a global ref to the NativeActivity jobject from `activity_as_ptr`.
pub(crate) fn set_activity(activity: *mut c_void) {
    let global = with_env(|env| {
        env.new_global_ref(unsafe { JObject::from_raw(activity as jni::sys::jobject) })
    });
    match global {
        Some(g) => *ACTIVITY.lock().unwrap() = Some(g),
        None => log::error!("failed to store activity global ref"),
    }
}

// Local frame: android_main stays attached for the process lifetime, so
// unscoped local refs would accumulate in its reference table forever.
pub(crate) fn with_env<R>(
    f: impl FnOnce(&mut jni::JNIEnv) -> jni::errors::Result<R>,
) -> Option<R> {
    let ctx = ndk_context::android_context();
    let vm = unsafe { JavaVM::from_raw(ctx.vm().cast()) }.ok()?;
    let mut env = vm.attach_current_thread().ok()?;
    match env.with_local_frame(32, |env| f(env)) {
        Ok(r) => Some(r),
        Err(e) => {
            let _ = env.exception_clear();
            log::error!("JNI error: {e:?}");
            None
        }
    }
}

/// Runs `f` with the Application context object.
pub(crate) fn with_context<R>(
    f: impl FnOnce(&mut jni::JNIEnv, &JObject) -> jni::errors::Result<R>,
) -> Option<R> {
    let ctx = ndk_context::android_context();
    with_env(|env| {
        let context = unsafe { JObject::from_raw(ctx.context().cast()) };
        f(env, &context)
    })
}

/// Runs `f` with the Activity object; None until `set_activity` has run.
pub(crate) fn with_activity<R>(
    f: impl FnOnce(&mut jni::JNIEnv, &JObject) -> jni::errors::Result<R>,
) -> Option<R> {
    let activity = ACTIVITY.lock().unwrap().clone()?;
    with_env(|env| f(env, activity.as_obj()))
}
