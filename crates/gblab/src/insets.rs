//! Status/navigation bar heights via JNI for Android 15+ edge-to-edge.

use std::sync::OnceLock;

use jni::JavaVM;
use jni::objects::{JObject, JValue};

static INSETS_PX: OnceLock<(f32, f32)> = OnceLock::new();

/// (top, bottom) safe-area insets in egui points.
pub fn safe_area(pixels_per_point: f32) -> (f32, f32) {
    let (t, b) = *INSETS_PX.get_or_init(|| read_insets_px().unwrap_or((0.0, 0.0)));
    let p = pixels_per_point.max(0.1);
    (t / p, b / p)
}

fn with_activity<R>(
    f: impl FnOnce(&mut jni::JNIEnv, &JObject) -> jni::errors::Result<R>,
) -> Option<R> {
    let ctx = ndk_context::android_context();
    let vm = unsafe { JavaVM::from_raw(ctx.vm().cast()) }.ok()?;
    let mut env = vm.attach_current_thread().ok()?;
    let activity = unsafe { JObject::from_raw(ctx.context().cast()) };
    match f(&mut env, &activity) {
        Ok(r) => Some(r),
        Err(e) => {
            let _ = env.exception_clear();
            log::error!("insets JNI error: {e:?}");
            None
        }
    }
}

// Resources lookups are thread-safe; View inset APIs must not run off the UI thread.
fn read_insets_px() -> Option<(f32, f32)> {
    with_activity(|env, activity| {
        let res = env
            .call_method(activity, "getResources", "()Landroid/content/res/Resources;", &[])?
            .l()?;
        let top = android_dimen_px(env, &res, "status_bar_height")?;
        let bottom = android_dimen_px(env, &res, "navigation_bar_height")?;
        Ok((top, bottom))
    })
}

/// Framework `dimen` resource in pixels; 0 if absent.
fn android_dimen_px(
    env: &mut jni::JNIEnv,
    res: &JObject,
    name: &str,
) -> jni::errors::Result<f32> {
    let jname = env.new_string(name)?;
    let jtype = env.new_string("dimen")?;
    let jpkg = env.new_string("android")?;
    let id = env
        .call_method(
            res,
            "getIdentifier",
            "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)I",
            &[(&jname).into(), (&jtype).into(), (&jpkg).into()],
        )?
        .i()?;
    if id <= 0 {
        return Ok(0.0);
    }
    let px = env.call_method(res, "getDimensionPixelSize", "(I)I", &[JValue::Int(id)])?.i()?;
    Ok(px as f32)
}
