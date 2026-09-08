// Windows shell helpers with no Q_OBJECT of their own.
//
// Compiled on EVERY platform on purpose: the bodies are portable Qt and only
// the caller's use of the WId is Windows-specific. Keeping it unconditional
// means the file is type-checked by the Linux and macOS builds too, which is
// where a signature drift would otherwise go unnoticed until a Windows CI run.

#include <QtCore/QObject>
#include <QtGui/QGuiApplication>
#include <QtGui/QSessionManager>
#include <QtGui/QWindow>

#ifdef _WIN32
#include <QtCore/QAbstractNativeEventFilter>
#include <QtCore/QByteArray>
#include <QtCore/QCoreApplication>

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <windows.h>
#endif

// The top-level window's native handle, or nullptr before one is shown.
//
// MUST NOT be called before exec(): QWindow::winId() CREATES the platform
// window as a side effect, so an early call would hand back a handle for a
// window Qt has not finished setting up, and the real one would then get a
// different HWND. Callers hop to the GUI thread first; winId() is not
// thread-safe.
extern "C" void *qbz_main_window_hwnd()
{
    const auto windows = QGuiApplication::topLevelWindows();
    for (QWindow *w : windows)
        if (w->isVisible() && w->type() == Qt::Window)
            return reinterpret_cast<void *>(w->winId());
    return nullptr;
}

#ifdef _WIN32
namespace {

// Give the custom-chrome window a real hit-test back.
//
// MEASURED, not assumed. With `Qt::CustomizeWindowHint` -- which is the only
// way to stop Qt painting its own minimise/maximise/close cluster on top of
// QBZ's header -- Qt answers WM_NCHITTEST with HTNOWHERE for the ENTIRE
// window: centre, header, player bar and sidebar all returned 0. Windows
// delivers no mouse input to HTNOWHERE, so the app looked frozen while it was
// running perfectly and the tray menu still drove playback.
//
// Without that hint Qt hit-tests correctly but populates the default title
// hints, and its customized-title path draws the buttons over our header. No
// combination of public flags gives all three of: no drawn buttons, working
// clicks, and the WS_THICKFRAME that Windows tiling needs.
//
// So the flags keep CustomizeWindowHint and this filter answers the hit test
// the way an ordinary sizable, caption-less window would: DefWindowProcW
// returns the eight edge codes near the frame and HTCLIENT everywhere else.
// Nothing here invents geometry -- it defers to the same code every other
// borderless-resizable window uses.
//
// Dragging is unaffected and stays with QML's `startSystemMove()`: with no
// caption there is no HTCAPTION band to inherit, which is the arrangement the
// header was already written for.
class QbzNcHitTestFilter : public QAbstractNativeEventFilter
{
public:
    bool nativeEventFilter(const QByteArray &type, void *message, qintptr *result) override
    {
        if (type != QByteArrayLiteral("windows_generic_MSG"))
            return false;
        MSG *msg = static_cast<MSG *>(message);
        if (!msg || msg->message != WM_NCHITTEST)
            return false;

        // NARROW ON PURPOSE: only a window that is sizable AND caption-less,
        // which is exactly the custom-chrome main window. Menus, tooltips and
        // popups have no WS_THICKFRAME; a window showing the system title bar
        // has WS_CAPTION and must keep Qt's own answer. Getting this wrong
        // would break hit-testing everywhere instead of fixing it in one
        // place.
        const LONG_PTR style = GetWindowLongPtrW(msg->hwnd, GWL_STYLE);
        if ((style & WS_THICKFRAME) == 0 || (style & WS_CAPTION) == WS_CAPTION)
            return false;

        *result = static_cast<qintptr>(
            DefWindowProcW(msg->hwnd, WM_NCHITTEST, msg->wParam, msg->lParam));
        return true;
    }
};

QbzNcHitTestFilter *g_hit_filter = nullptr;

}  // namespace
#endif  // _WIN32

// Install the hit-test filter. Idempotent; a no-op off Windows.
extern "C" void qbz_install_hittest_filter()
{
#ifdef _WIN32
    if (g_hit_filter)
        return;
    g_hit_filter = new QbzNcHitTestFilter;
    QCoreApplication::instance()->installNativeEventFilter(g_hit_filter);
#endif
}

// WM_QUERYENDSESSION -> QGuiApplication::commitDataRequest, via Qt's Windows
// session manager. It fires BEFORE Windows decides whether to proceed with the
// logoff and Qt blocks until the handler returns, which makes it the one safe
// place to persist synchronously: anything deferred to a queued connection or
// another thread races the process being killed.
extern "C" void qbz_install_commit_data_handler(void (*cb)())
{
#ifndef QT_NO_SESSIONMANAGER
    // Once. Qt::UniqueConnection does NOT deduplicate lambdas -- each connect
    // makes an independent connection -- so a second install would run the
    // session persist twice on every logoff.
    static bool installed = false;
    if (installed)
        return;
    installed = true;

    QObject::connect(qApp, &QGuiApplication::commitDataRequest, qApp,
                     [cb](QSessionManager &) { cb(); });
#else
    // A Qt built without the session manager has no commitDataRequest at all,
    // and this file compiles on every platform.
    (void)cb;
#endif
}
