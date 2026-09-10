/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use servo_base::generic_channel::GenericCallback;

use crate::WebView;

pub struct StringRequest {
    pub(crate) result_sender: GenericCallback<Result<String, String>>,
    response_sent: bool,
}

impl StringRequest {
    pub fn success(mut self, string: String) {
        let _ = self.result_sender.send(Ok(string));
        self.response_sent = true;
    }

    pub fn failure(mut self, message: String) {
        let _ = self.result_sender.send(Err(message));
        self.response_sent = true;
    }
}

impl From<GenericCallback<Result<String, String>>> for StringRequest {
    fn from(result_sender: GenericCallback<Result<String, String>>) -> Self {
        Self {
            result_sender,
            response_sent: false,
        }
    }
}

impl Drop for StringRequest {
    fn drop(&mut self) {
        if !self.response_sent {
            let _ = self
                .result_sender
                .send(Err("No response sent to request.".into()));
        }
    }
}

/// A delegate that is responsible for accessing the system clipboard. On Mac, Windows, and
/// Linux if the `clipboard` feature is enabled, a default delegate is automatically used
/// that implements clipboard support. An embedding application can override this delegate
/// by using this trait.
pub trait ClipboardDelegate {
    /// A request to clear all contents of the system clipboard.
    fn clear(&self, _webview: WebView) {}

    /// A request to get the text contents of the system clipboard. Once the contents are
    /// retrieved the embedder should call [`StringRequest::success`] with the text or
    /// [`StringRequest::failure`] with a failure message.
    fn get_text(&self, _webview: WebView, _request: StringRequest) {}

    /// A request to set the text contents of the system clipboard to `new_contents`.
    fn set_text(&self, _webview: WebView, _new_contents: String) {}
}

pub(crate) struct DefaultClipboardDelegate;

impl ClipboardDelegate for DefaultClipboardDelegate {
    fn clear(&self, _webview: WebView) {
        clipboard::clear();
    }

    fn get_text(&self, _webview: WebView, request: StringRequest) {
        clipboard::get_text(request);
    }

    fn set_text(&self, _webview: WebView, new_contents: String) {
        clipboard::set_text(new_contents);
    }
}

mod fallback_clipboard {
    use std::sync::{LockResult, Mutex, OnceLock};

    use crate::clipboard_delegate::StringRequest;

    /// If the clipboard cannot be accessed, we fall back to a simple `String` to store
    /// text for the clipboard. This obviously does not work across processes.
    static SHARED_FALLBACK_CLIPBOARD: OnceLock<Mutex<String>> = OnceLock::new();

    fn with_shared_clipboard(callback: impl FnOnce(&mut String)) {
        let clipboard_mutex =
            SHARED_FALLBACK_CLIPBOARD.get_or_init(|| Mutex::new(Default::default()));
        if let LockResult::Ok(mut string) = clipboard_mutex.lock() {
            callback(&mut string)
        }
    }

    pub(super) fn clear() {
        with_shared_clipboard(|clipboard_string| {
            clipboard_string.clear();
        });
    }

    pub(super) fn get_text(request: StringRequest) {
        with_shared_clipboard(move |clipboard_string| request.success(clipboard_string.clone()));
    }

    pub(super) fn set_text(new_contents: String) {
        with_shared_clipboard(move |clipboard_string| {
            *clipboard_string = new_contents;
        });
    }
}

#[cfg(all(
    feature = "clipboard",
    not(any(target_os = "android", target_env = "ohos"))
))]
mod clipboard {
    use std::sync::OnceLock;

    use arboard::Clipboard;
    use parking_lot::Mutex;

    use super::StringRequest;
    use crate::clipboard_delegate::fallback_clipboard;

    /// A shared clipboard for use by the [`DefaultClipboardDelegate`](super::DefaultClipboardDelegate).
    /// This is protected by a mutex so that it can only be used by one thread at a time.
    /// The `arboard` documentation suggests that more than one thread shouldn't try to access
    /// the Windows clipboard at a time. See <https://docs.rs/arboard/latest/arboard/struct.Clipboard.html>.
    static SHARED_CLIPBOARD: OnceLock<Option<Mutex<Clipboard>>> = OnceLock::new();

    fn with_shared_clipboard<ResultType>(
        callback: impl FnOnce(&mut Clipboard) -> Result<ResultType, arboard::Error>,
    ) -> Result<ResultType, arboard::Error> {
        match SHARED_CLIPBOARD.get_or_init(|| Clipboard::new().ok().map(Mutex::new)) {
            Some(clipboard_mutex) => callback(&mut clipboard_mutex.lock()),
            None => Err(arboard::Error::ClipboardNotSupported),
        }
    }

    pub(super) fn clear() {
        if with_shared_clipboard(|clipboard| clipboard.clear()).is_err() {
            fallback_clipboard::clear();
        }
    }

    pub(super) fn get_text(request: StringRequest) {
        if let Ok(text) = with_shared_clipboard(|clipboard| clipboard.get_text()) {
            request.success(text);
            return;
        };
        fallback_clipboard::get_text(request);
    }

    pub(super) fn set_text(new_contents: String) {
        if with_shared_clipboard(|clipboard| clipboard.set_text(&new_contents)).is_err() {
            fallback_clipboard::set_text(new_contents);
        }
    }
}

#[cfg(all(feature = "clipboard", target_env = "ohos"))]
mod clipboard {
    use super::StringRequest;
    use crate::clipboard_delegate::fallback_clipboard;

    pub(super) fn clear() {
        if let Err(error) = ohos_pasteboard::clear() {
            log::warn!(
                "OHOS pasteboard clear failed ({error}); using in-memory fallback_clipboard"
            );
            fallback_clipboard::clear();
        }
    }

    pub(super) fn get_text(request: StringRequest) {
        match ohos_pasteboard::get_text() {
            Ok(text) => request.success(text),
            Err(ohos_pasteboard::Error::NoText) => request.success(String::new()),
            Err(error) => {
                log::warn!(
                    "OHOS pasteboard get_text failed ({error}); using in-memory fallback_clipboard"
                );
                fallback_clipboard::get_text(request);
            },
        }
    }

    pub(super) fn set_text(new_contents: String) {
        if let Err(error) = ohos_pasteboard::set_text(&new_contents) {
            log::warn!(
                "OHOS pasteboard set_text failed ({error}); using in-memory fallback_clipboard"
            );
            fallback_clipboard::set_text(new_contents);
        }
    }
}

#[cfg(all(feature = "clipboard", target_env = "android"))]
mod clipboard {
    use jni::errors::{Error, ThrowRuntimeExAndDefault};
    use jni::objects::{Global, JClass, JObject, JString, JValue, JValueOwned};
    use jni::strings::JNIStr;
    use jni::sys::{jboolean, jfloat, jint, jobject};
    use jni::{Env, EnvUnowned, JavaVM, jni_sig, jni_str};

    use super::StringRequest;
    use crate::clipboard_delegate::fallback_clipboard;

    fn with_clipboard_access<F, T>(callback: F) -> Result<T, Error>
    where
        F: FnOnce(&mut Env, JObject) -> Result<T, Error>,
    {
        let ctx = ndk_context::android_context();

        let jvm = unsafe { JavaVM::from_raw(ctx.vm().cast()) };

        jvm.attach_current_thread(|env| {
            let context = unsafe { JObject::from_raw(env, ctx.context().cast()) };
            let clipboard = env.new_string("clipboard")?;

            let clipboard_manager = env
                .call_method(
                    context,
                    jni_str!("getSystemService"),
                    jni_sig!("(Ljava/lang/String;)Ljava/lang/Object;"),
                    &[(&clipboard).into()],
                )?
                .l()?;

            callback(env, clipboard_manager)
        })
    }

    pub(super) fn clear() {
        let result = with_clipboard_access(|env, clipboard_manager| {
            env.call_method(
                clipboard_manager,
                jni_str!("clearPrimaryClip"),
                jni_sig!("()V"),
                &[],
            )
        });
        if let Err(error) = result {
            log::warn!(
                "OHOS pasteboard clear failed ({error}); using in-memory fallback_clipboard"
            );
            fallback_clipboard::clear();
        }
    }

    pub(super) fn get_text(request: StringRequest) {
        with_clipboard_access(|env, clipboard_manager| {
            if !env
                .call_method(
                    &clipboard_manager,
                    jni_str!("hasPrimaryClip"),
                    jni_sig!("()Z"),
                    &[],
                )?
                .z()?
            {
                return request.success(String::new());
            }

            let clip = env
                .call_method(
                    clipboard_manager,
                    jni_str!("getPrimaryClip"),
                    jni_sig!("()Landroid/content/ClipData;"),
                    &[],
                )?
                .l()?;

            if env
                .call_method(&clip, jni_str!("getItemCount"), jni_sig!("()I"), &[])?
                .i()? ==
                0
            {
                return request.success(String::new());
            }

            let item = env
                .call_method(
                    &clip,
                    jni_str!("getItemAt"),
                    jni_sig!("(I)Landroid/content/ClipData$Item;"),
                    &[0.into()],
                )?
                .l()?;

            let char_sequence = env
                .call_method(
                    item,
                    jni_str!("getText"),
                    jni_sig!("()Ljava/lang/CharSequence;"),
                    &[],
                )?
                .l()?;
            let text = env.cast_local::<JString>(char_sequence)?.to_string();

            return request.success(text);
        });
    }

    pub(super) fn set_text(new_contents: String) {}
}

#[cfg(not(feature = "clipboard"))]
mod clipboard {
    use super::StringRequest;
    use crate::clipboard_delegate::fallback_clipboard;

    pub(super) fn clear() {
        fallback_clipboard::clear();
    }

    pub(super) fn get_text(request: StringRequest) {
        fallback_clipboard::get_text(request);
    }

    pub(super) fn set_text(new_contents: String) {
        fallback_clipboard::set_text(new_contents);
    }
}
