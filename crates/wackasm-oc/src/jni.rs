//! JNI entry points for `dev.wackasm.oc.WackasmNative`.
//!
//! Rust never calls back into Java, and no JNI references outlive a call.
//! Every interaction is a call that Java starts, carrying primitives and
//! `byte[]`s in the [`crate::value`] encoding. That keeps the JNI surface
//! small and makes it safe to call from any thread.
//!
//! A machine handle is a `Box<Mutex<OcMachine>>` turned into a `jlong`.
//! OpenComputers never runs one machine on two threads at once, but the
//! mutex makes a mistake there harmless instead of undefined behaviour.
//! Errors and panics turn into Java exceptions and never unwind across the
//! FFI boundary.

use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::Mutex,
};

use anyhow::{Result, anyhow};
use jni::{
    JNIEnv,
    objects::{JByteArray, JClass, JString},
    sys::{JNI_FALSE, JNI_TRUE, jboolean, jbyteArray, jdouble, jint, jlong},
};

use crate::{machine::OcMachine, storage::WorkDir};

type Handle = Mutex<OcMachine>;

const ERROR_CLASS: &str = "java/lang/IllegalStateException";

/// Runs `f`, turning `Err` and panics into a pending Java exception and
/// returning `fallback` to the JVM. Java ignores that value because the
/// exception is thrown.
fn guard<'l, T>(env: &mut JNIEnv<'l>, fallback: T, f: impl FnOnce(&mut JNIEnv<'l>) -> Result<T>) -> T {
    let message = match catch_unwind(AssertUnwindSafe(|| f(env))) {
        Ok(Ok(value)) => return value,
        Ok(Err(error)) => format!("{error:#}"),
        Err(panic) => {
            let detail = panic
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".into());
            format!("wackasm-oc panicked: {detail}")
        }
    };
    if !env.exception_check().unwrap_or(false) {
        let _ = env.throw_new(ERROR_CLASS, message);
    }
    fallback
}

fn with_machine<T>(handle: jlong, f: impl FnOnce(&mut OcMachine) -> Result<T>) -> Result<T> {
    if handle == 0 {
        return Err(anyhow!("machine handle is null (already destroyed?)"));
    }
    // SAFETY: non-zero handles come from `create` and stay valid until `destroy`.
    let handle = unsafe { &*(handle as *const Handle) };
    let mut machine = handle
        .lock()
        .map_err(|_| anyhow!("machine poisoned by an earlier panic"))?;
    f(&mut machine)
}

fn byte_array(env: &mut JNIEnv, bytes: Option<Vec<u8>>) -> Result<jbyteArray> {
    Ok(match bytes {
        Some(bytes) => env.byte_array_from_slice(&bytes)?.into_raw(),
        None => std::ptr::null_mut(),
    })
}

fn optional_bytes(env: &mut JNIEnv, array: &JByteArray) -> Result<Option<Vec<u8>>> {
    if array.is_null() {
        Ok(None)
    } else {
        Ok(Some(env.convert_byte_array(array)?))
    }
}

/// `static native long create(String workDir, String key, String address)`
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_wackasm_oc_WackasmNative_create<'l>(
    mut env: JNIEnv<'l>,
    _class: JClass<'l>,
    work_dir: JString<'l>,
    key: JString<'l>,
    address: JString<'l>,
) -> jlong {
    guard(&mut env, 0, |env| {
        let work_dir: String = env.get_string(&work_dir)?.into();
        let key: String = env.get_string(&key)?.into();
        let address: String = env.get_string(&address)?.into();
        let machine = OcMachine::new(WorkDir::open(work_dir)?, &key, &address)?;
        Ok(Box::into_raw(Box::new(Mutex::new(machine))) as jlong)
    })
}

/// `static native void destroy(long handle)`. Destroying a null handle (0) does nothing.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_wackasm_oc_WackasmNative_destroy<'l>(
    mut env: JNIEnv<'l>,
    _class: JClass<'l>,
    handle: jlong,
) {
    guard(&mut env, (), |_| {
        if handle != 0 {
            // SAFETY: produced by `create`; Java zeroes its copy after destroying.
            drop(unsafe { Box::from_raw(handle as *mut Handle) });
        }
        Ok(())
    })
}

/// `static native void setMemoryLimit(long handle, long bytes)`
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_wackasm_oc_WackasmNative_setMemoryLimit<'l>(
    mut env: JNIEnv<'l>,
    _class: JClass<'l>,
    handle: jlong,
    bytes: jlong,
) {
    guard(&mut env, (), |_| {
        with_machine(handle, |m| Ok(m.set_memory_limit(bytes.max(0) as u64)))
    })
}

/// `static native void boot(long handle, byte[] eepromOrNull)`
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_wackasm_oc_WackasmNative_boot<'l>(
    mut env: JNIEnv<'l>,
    _class: JClass<'l>,
    handle: jlong,
    eeprom: JByteArray<'l>,
) {
    guard(&mut env, (), |env| {
        let eeprom = optional_bytes(env, &eeprom)?;
        with_machine(handle, |m| m.boot(eeprom.as_deref()))
    })
}

/// `static native int run(long handle, double uptime, long fuel)`.
/// Returns a `RUN_*` code; `payload` gives the details.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_wackasm_oc_WackasmNative_run<'l>(
    mut env: JNIEnv<'l>,
    _class: JClass<'l>,
    handle: jlong,
    uptime: jdouble,
    fuel: jlong,
) -> jint {
    guard(&mut env, crate::abi::RUN_ERROR, |_| {
        let fuel = isize::try_from(fuel.max(1)).unwrap_or(isize::MAX);
        with_machine(handle, |m| Ok(m.run(uptime, fuel)?.code()))
    })
}

/// `static native byte[] payload(long handle)`: the encoded array for the
/// pending request or halt reason, or `null` if there's nothing to act on.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_wackasm_oc_WackasmNative_payload<'l>(
    mut env: JNIEnv<'l>,
    _class: JClass<'l>,
    handle: jlong,
) -> jbyteArray {
    guard(&mut env, std::ptr::null_mut(), |env| {
        let payload = with_machine(handle, |m| Ok(m.current_payload().map(|v| v.encode())))?;
        byte_array(env, payload)
    })
}

/// `static native void respond(long handle, int status, byte[] payloadOrNull)`
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_wackasm_oc_WackasmNative_respond<'l>(
    mut env: JNIEnv<'l>,
    _class: JClass<'l>,
    handle: jlong,
    status: jint,
    payload: JByteArray<'l>,
) {
    guard(&mut env, (), |env| {
        let payload = optional_bytes(env, &payload)?.unwrap_or_default();
        with_machine(handle, |m| m.respond(status, payload))
    })
}

/// `static native void save(long handle)`: writes the snapshot into the work dir.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_wackasm_oc_WackasmNative_save<'l>(
    mut env: JNIEnv<'l>,
    _class: JClass<'l>,
    handle: jlong,
) {
    guard(&mut env, (), |_| with_machine(handle, |m| m.save()))
}

/// `static native boolean load(long handle)`: returns `false` when no snapshot exists.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_wackasm_oc_WackasmNative_load<'l>(
    mut env: JNIEnv<'l>,
    _class: JClass<'l>,
    handle: jlong,
) -> jboolean {
    guard(&mut env, JNI_FALSE, |_| {
        with_machine(handle, |m| Ok(if m.load()? { JNI_TRUE } else { JNI_FALSE }))
    })
}

/// `static native void discard(long handle)`: drops the program and deletes its snapshot.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_wackasm_oc_WackasmNative_discard<'l>(
    mut env: JNIEnv<'l>,
    _class: JClass<'l>,
    handle: jlong,
) {
    guard(&mut env, (), |_| with_machine(handle, |m| m.discard()))
}
