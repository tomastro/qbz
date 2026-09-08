#include <QtCore/QByteArray>
#include <QtCore/QCoreApplication>
#include <QtCore/QEventLoop>
#include <QtCore/QJsonArray>
#include <QtCore/QJsonDocument>
#include <QtCore/QJsonObject>
#include <QtCore/QTimer>
#include <QtCore/QVector>
#include <QtCore/QVersionNumber>
#include <QtGui/QGuiApplication>
#include <QtGui/qtguiglobal.h>
#include <QtQuick/QQuickWindow>

#if QT_CONFIG(vulkan) && __has_include(<vulkan/vulkan.h>)
#include <QtGui/QVulkanFunctions>
#include <private/qvulkandefaultinstance_p.h>
#endif

namespace {

QByteArray serializedDevices("[]");

#if QT_CONFIG(vulkan) && __has_include(<vulkan/vulkan.h>)
QByteArray deviceUuid(QVulkanFunctions *functions, VkPhysicalDevice device,
                      const QVulkanInstance *instance)
{
    if (instance->apiVersion() < QVersionNumber(1, 1))
        return {};

    VkPhysicalDeviceIDProperties id{};
    id.sType = VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_ID_PROPERTIES;
    VkPhysicalDeviceProperties2 properties{};
    properties.sType = VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_PROPERTIES_2;
    properties.pNext = &id;
    functions->vkGetPhysicalDeviceProperties2(device, &properties);

    bool any = false;
    for (const auto byte : id.deviceUUID)
        any = any || byte != 0;
    if (!any)
        return {};
    return QByteArray(reinterpret_cast<const char *>(id.deviceUUID), VK_UUID_SIZE).toHex();
}
#endif

} // namespace

// Enumerate through the very QVulkanInstance Qt Quick will later hand to
// QRhi. This is intentionally not a second raw vkCreateInstance: implicit
// layers (notably hybrid-GPU layers) may expose a different order depending
// on the instance extensions and platform integration. Returning Qt's exact
// order makes QT_VK_PHYSICAL_DEVICE_INDEX an identity translation instead of
// a guess.
extern "C" const char *qbz_qt_vulkan_devices_json()
{
    serializedDevices = "[]";

#if QT_CONFIG(vulkan) && __has_include(<vulkan/vulkan.h>)
    // QVulkanDefaultInstance needs the platform integration installed by
    // QGuiApplication. Rust calls this helper after constructing it and before
    // constructing the first QQuickWindow.
    if (QCoreApplication::instance() == nullptr)
        return serializedDevices.constData();

    QVulkanInstance *instance = QVulkanDefaultInstance::instance();
    if (instance == nullptr || !instance->isValid())
        return serializedDevices.constData();

    QVulkanFunctions *functions = instance->functions();
    if (functions == nullptr)
        return serializedDevices.constData();

    uint32_t count = 0;
    if (functions->vkEnumeratePhysicalDevices(instance->vkInstance(), &count, nullptr)
            != VK_SUCCESS || count == 0) {
        return serializedDevices.constData();
    }

    QVector<VkPhysicalDevice> devices(static_cast<qsizetype>(count));
    if (functions->vkEnumeratePhysicalDevices(instance->vkInstance(), &count, devices.data())
            != VK_SUCCESS) {
        return serializedDevices.constData();
    }

    QJsonArray result;
    for (uint32_t index = 0; index < count; ++index) {
        VkPhysicalDeviceProperties properties{};
        functions->vkGetPhysicalDeviceProperties(devices.at(index), &properties);

        QJsonObject item;
        item.insert(QStringLiteral("index"), static_cast<int>(index));
        item.insert(QStringLiteral("name"),
                    QString::fromUtf8(properties.deviceName).trimmed());
        item.insert(QStringLiteral("vendor"), static_cast<qint64>(properties.vendorID));
        item.insert(QStringLiteral("device"), static_cast<qint64>(properties.deviceID));
        item.insert(QStringLiteral("type"), static_cast<int>(properties.deviceType));
        item.insert(QStringLiteral("uuid"),
                    QString::fromLatin1(deviceUuid(functions, devices.at(index), instance)));
        result.append(item);
    }
    serializedDevices = QJsonDocument(result).toJson(QJsonDocument::Compact);
#endif

    return serializedDevices.constData();
}

// Exercise the exact path the real shell needs, but inside the disposable
// preflight child: a Qt Quick Vulkan swapchain has to present actual frames to
// the active QPA compositor. Merely creating VkDevice succeeds on the owner's
// unsupported Intel-render -> NVIDIA/KWin path; the fatal failure only arrives
// when Wayland imports the first DMA-BUF. If that protocol error kills this
// process, the parent remains alive and falls back to Auto.
static int presentationPreflight(bool autoBackend)
{
    if (qobject_cast<QGuiApplication *>(QCoreApplication::instance()) == nullptr)
        return 71;

    QQuickWindow window;
    window.setTitle(QStringLiteral("QBZ graphics preflight"));
    window.setFlags(Qt::Tool | Qt::FramelessWindowHint | Qt::WindowDoesNotAcceptFocus);
    window.resize(2, 2);
    window.setColor(Qt::transparent);
    // Keep the native surface effectively invisible while still requiring a
    // real buffer commit/import. An offscreen window would miss the exact
    // Wayland failure this probe exists to catch.
    window.setOpacity(0.01);

    QEventLoop loop;
    QTimer pulse;
    QTimer settle;
    QTimer deadline;
    pulse.setInterval(16);
    settle.setSingleShot(true);
    deadline.setSingleShot(true);

    int frames = 0;
    int result = 76; // hard deadline / no usable presentation

    QObject::connect(&pulse, &QTimer::timeout, &window, &QQuickWindow::update);
    QObject::connect(&window, &QQuickWindow::frameSwapped, &loop, [&] {
        ++frames;
        if (autoBackend && frames >= 3) {
            // Pump successive native presentations, not just context creation.
            // Auto has no fixed grace sleep on the healthy startup path.
            result = 0;
            loop.quit();
        } else if (!autoBackend && frames == 1) {
            settle.start(700);
        }
        window.update();
    });
    QObject::connect(&window, &QQuickWindow::sceneGraphError, &loop,
                     [&](QQuickWindow::SceneGraphError, const QString &) {
        result = 74;
        loop.quit();
    });
    QObject::connect(&settle, &QTimer::timeout, &loop, [&] {
        // Two swaps plus a grace interval keep the Wayland connection pumping
        // long enough for an asynchronous compositor import error to arrive.
        result = frames >= 2 ? 0 : 75;
        loop.quit();
    });
    QObject::connect(&deadline, &QTimer::timeout, &loop, [&] {
        result = 76;
        loop.quit();
    });

    window.show();
    window.update();
    pulse.start();
    deadline.start(5000);
    loop.exec();

    pulse.stop();
    settle.stop();
    deadline.stop();
    window.hide();
    window.releaseResources();
    QCoreApplication::processEvents(QEventLoop::AllEvents, 50);
    return result;
}

extern "C" int qbz_qt_vulkan_preflight_window()
{
    return presentationPreflight(false);
}

// Uses Qt's actual default (OpenGL on Linux), or software selected BEFORE
// QGuiApplication. No Vulkan inventory or multiple-adapter requirement.
extern "C" int qbz_qt_auto_preflight_window()
{
    return presentationPreflight(true);
}
