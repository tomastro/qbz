//! QbzLink — state and actions for the app-wide "Open Music Link" modal.

use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use cxx_qt::CxxQtThread;
use cxx_qt::Threading as _;
use cxx_qt_lib::QString;

#[cxx_qt::bridge]
pub mod qbz_link {
    extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    #[auto_cxx_name]
    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        #[qproperty(bool, modal_open)]
        #[qproperty(bool, resolving)]
        #[qproperty(QString, url)]
        #[qproperty(QString, platform)]
        #[qproperty(QString, error)]
        #[qproperty(bool, playlist_detected)]
        #[qproperty(QString, playlist_provider)]
        type QbzLink = super::QbzLinkRust;

        #[qinvokable]
        fn boot(self: Pin<&mut QbzLink>);
        #[qinvokable]
        fn show(self: Pin<&mut QbzLink>);
        #[qinvokable]
        fn close(self: Pin<&mut QbzLink>);
        #[qinvokable]
        fn url_edited(self: Pin<&mut QbzLink>, url: QString);
        #[qinvokable]
        fn submit(self: Pin<&mut QbzLink>, url: QString);
        #[qinvokable]
        fn open_importer(self: Pin<&mut QbzLink>);
    }

    impl cxx_qt::Threading for QbzLink {}
}

use qbz_link::QbzLink;

pub struct QbzLinkRust {
    modal_open: bool,
    resolving: bool,
    url: QString,
    platform: QString,
    error: QString,
    playlist_detected: bool,
    playlist_provider: QString,
}

impl Default for QbzLinkRust {
    fn default() -> Self {
        Self {
            modal_open: false,
            resolving: false,
            url: QString::default(),
            platform: QString::default(),
            error: QString::default(),
            playlist_detected: false,
            playlist_provider: QString::default(),
        }
    }
}

static QT_THREAD: OnceLock<CxxQtThread<QbzLink>> = OnceLock::new();
static OPEN: AtomicBool = AtomicBool::new(false);

pub(crate) fn ui(f: impl FnOnce(Pin<&mut QbzLink>) + Send + 'static) {
    if let Some(thread) = QT_THREAD.get() {
        let _ = thread.queue(f);
    }
}

pub(crate) fn is_open() -> bool {
    OPEN.load(Ordering::SeqCst)
}

pub(crate) fn open_modal() {
    OPEN.store(true, Ordering::SeqCst);
    ui(reset_open);
}

pub(crate) fn close_modal() {
    OPEN.store(false, Ordering::SeqCst);
    ui(|mut link| {
        link.as_mut().set_modal_open(false);
    });
}

fn reset_open(mut link: Pin<&mut QbzLink>) {
    link.as_mut().set_url(QString::default());
    link.as_mut().set_platform(QString::default());
    link.as_mut().set_error(QString::default());
    link.as_mut().set_playlist_detected(false);
    link.as_mut().set_playlist_provider(QString::default());
    link.as_mut().set_resolving(false);
    link.as_mut().set_modal_open(true);
}

impl qbz_link::QbzLink {
    pub fn boot(self: Pin<&mut Self>) {
        if QT_THREAD.set(self.qt_thread()).is_err() {
            log::warn!("[qbz-qt] link resolver Qt thread already registered");
        }
    }

    pub fn show(self: Pin<&mut Self>) {
        OPEN.store(true, Ordering::SeqCst);
        reset_open(self);
    }

    pub fn close(mut self: Pin<&mut Self>) {
        OPEN.store(false, Ordering::SeqCst);
        self.as_mut().set_modal_open(false);
    }

    pub fn url_edited(mut self: Pin<&mut Self>, url: QString) {
        let platform = crate::link_resolver_qt::detect_platform(&url.to_string());
        self.as_mut().set_url(url);
        self.as_mut().set_platform(QString::from(platform));
        self.as_mut().set_error(QString::default());
        self.as_mut().set_playlist_detected(false);
        self.as_mut().set_playlist_provider(QString::default());
    }

    pub fn submit(mut self: Pin<&mut Self>, url: QString) {
        let url = url.to_string().trim().to_string();
        if url.is_empty() || *self.as_ref().resolving() {
            return;
        }
        self.as_mut().set_url(QString::from(url.as_str()));
        self.as_mut().set_resolving(true);
        self.as_mut().set_error(QString::default());
        self.as_mut().set_playlist_detected(false);
        self.as_mut().set_playlist_provider(QString::default());

        let runtime = crate::app();
        crate::spawn(async move {
            let result = crate::link_resolver_qt::resolve(runtime, url).await;
            ui(move |mut link| {
                link.as_mut().set_resolving(false);
                match result {
                    Ok(crate::link_resolver_qt::Outcome::Resolved(target)) => {
                        OPEN.store(false, Ordering::SeqCst);
                        link.as_mut().set_modal_open(false);
                        crate::link_resolver_qt::navigate(target);
                    }
                    Ok(crate::link_resolver_qt::Outcome::PlaylistDetected(provider)) => {
                        link.as_mut().set_playlist_detected(true);
                        link.as_mut()
                            .set_playlist_provider(QString::from(provider.as_str()));
                    }
                    Ok(crate::link_resolver_qt::Outcome::NotOnQobuz) => {
                        link.as_mut().set_error(QString::from(
                            qbz_i18n::t("This content is not available on Qobuz").as_str(),
                        ));
                    }
                    Err(e) => {
                        log::warn!("[qbz-qt] open-link resolve failed: {e}");
                        link.as_mut().set_error(QString::from(
                            qbz_i18n::t("Could not resolve that link").as_str(),
                        ));
                    }
                }
            });
        });
    }

    pub fn open_importer(mut self: Pin<&mut Self>) {
        OPEN.store(false, Ordering::SeqCst);
        self.as_mut().set_modal_open(false);
        crate::playlist_import_qt::open();
    }
}
