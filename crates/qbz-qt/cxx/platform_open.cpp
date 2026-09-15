#include <QtCore/QCoreApplication>
#include <QtCore/QMetaObject>
#include <QtCore/QThread>
#include <QtCore/QUrl>
#include <QtGui/QDesktopServices>

#include <cstddef>
#include <cstdlib>

#if defined(__ANDROID__) || defined(ANDROID)
#include <dlfcn.h>
#include <QtCore/QDir>
#include <QtQml/QQmlApplicationEngine>

static QString s_nativeLibDir;

extern "C" void qbz_qt_android_init_platform_plugin_path(const char *path_utf8)
{
    // Ensure Android application files and cache directories are set in libc environment
    // so dirs::data_dir() and dirs::cache_dir() in Rust resolve successfully.
    const char *filesDir = "/data/user/0/dev.qbz.android.qt/files";
    const char *cacheDir = "/data/user/0/dev.qbz.android.qt/cache";

    setenv("HOME", filesDir, 1);
    setenv("XDG_DATA_HOME", filesDir, 1);
    setenv("XDG_CACHE_HOME", cacheDir, 1);
    setenv("XDG_CONFIG_HOME", filesDir, 1);
    qputenv("HOME", filesDir);
    qputenv("XDG_DATA_HOME", filesDir);
    qputenv("XDG_CACHE_HOME", cacheDir);
    qputenv("XDG_CONFIG_HOME", filesDir);

    if (path_utf8 && *path_utf8) {
        s_nativeLibDir = QString::fromUtf8(path_utf8);
    } else {
        Dl_info info;
        if (dladdr(reinterpret_cast<const void *>(qbz_qt_android_init_platform_plugin_path), &info) && info.dli_fname) {
            QString libPath = QString::fromUtf8(info.dli_fname);
            int lastSlash = libPath.lastIndexOf(QLatin1Char('/'));
            if (lastSlash != -1) {
                s_nativeLibDir = libPath.left(lastSlash);
            }
        }
    }

    if (!s_nativeLibDir.isEmpty()) {
        qputenv("QT_QPA_PLATFORM", "android");
        qputenv("QT_PLUGIN_PATH", s_nativeLibDir.toUtf8());
        qputenv("QT_QPA_PLATFORM_PLUGIN_PATH", s_nativeLibDir.toUtf8());
        qputenv("QML_PLUGIN_PATH", s_nativeLibDir.toUtf8());
        qputenv("QML_IMPORT_PATH", s_nativeLibDir.toUtf8());
        qputenv("QML2_IMPORT_PATH", s_nativeLibDir.toUtf8());
        qputenv("QML_IMPORT_TRACE", "1");
        qputenv("QT_DEBUG_PLUGINS", "1");
        qputenv("QT_LOGGING_RULES", "qt.qml.import*=true;qt.core.plugin*=true;qt.qml.import.debug=true");
        QCoreApplication::addLibraryPath(s_nativeLibDir);
    }
}

extern "C" void qbz_qt_android_configure_qml_engine(void *engine_ptr)
{
    if (!engine_ptr) return;
    auto *engine = reinterpret_cast<QQmlApplicationEngine *>(engine_ptr);
    if (!s_nativeLibDir.isEmpty()) {
        engine->addPluginPath(s_nativeLibDir);
        engine->addImportPath(s_nativeLibDir);
    }
    engine->addImportPath(QStringLiteral("qrc:/qt-project.org/imports"));
    engine->addImportPath(QStringLiteral("qrc:/qt/qml"));
    engine->addImportPath(QStringLiteral("assets:/"));
    engine->addPluginPath(QStringLiteral("qrc:/qt-project.org/imports"));
    engine->addPluginPath(QStringLiteral("qrc:/qt/qml"));
}

extern "C" const char *qbz_qt_android_get_native_lib_dir()
{
    return s_nativeLibDir.toUtf8().constData();
}
#else
extern "C" void qbz_qt_android_init_platform_plugin_path(const char * /*path_utf8*/)
{
}
extern "C" void qbz_qt_android_configure_qml_engine(void * /*engine_ptr*/)
{
}
extern "C" const char *qbz_qt_android_get_native_lib_dir()
{
    return "";
}
#endif

extern "C" bool qbz_qt_open_target(const unsigned char *utf8, std::size_t len)
{
    if (!utf8 || len == 0)
        return false;

    const auto text = QString::fromUtf8(reinterpret_cast<const char *>(utf8),
                                        static_cast<qsizetype>(len));
    const auto url = QUrl::fromUserInput(text);
    if (!url.isValid())
        return false;

    auto *app = QCoreApplication::instance();
    if (!app || QThread::currentThread() == app->thread())
        return QDesktopServices::openUrl(url);

    bool opened = false;
    QMetaObject::invokeMethod(app, [&opened, url]() {
        opened = QDesktopServices::openUrl(url);
    }, Qt::BlockingQueuedConnection);
    return opened;
}
