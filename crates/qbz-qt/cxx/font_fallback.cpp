#include <QtCore/QChar>
#include <QtCore/QByteArray>
#include <QtCore/QMessageLogContext>
#include <QtCore/QString>
#include <QtCore/QStringList>
#include <QtCore/qlogging.h>
#include <QtGui/QFontDatabase>

#include <cstdio>

namespace {

QtMessageHandler previousMessageHandler = nullptr;
bool messageFilterInstalled = false;

bool isExpectedDevanagariCapabilityWarning(
    QtMsgType type, const QMessageLogContext &context, const QString &message)
{
    return type == QtWarningMsg
        && context.category != nullptr
        && qstrcmp(context.category, "qt.text.font.db") == 0
        && message.startsWith(QStringLiteral("OpenType support missing for \""))
        && message.endsWith(QStringLiteral(", script 11"));
}

void fontMessageHandler(
    QtMsgType type, const QMessageLogContext &context, const QString &message)
{
    // Qt probes the selected UI face before consulting the application
    // fallback. A face without Devanagari tables therefore emits this benign
    // diagnostic even though the registered Noto face renders the text. Keep
    // every other Qt diagnostic intact.
    if (isExpectedDevanagariCapabilityWarning(type, context, message))
        return;

    if (previousMessageHandler != nullptr) {
        previousMessageHandler(type, context, message);
        return;
    }

    const auto rendered = qFormatLogMessage(type, context, message).toLocal8Bit();
    std::fwrite(rendered.constData(), 1, rendered.size(), stderr);
    std::fputc('\n', stderr);
    std::fflush(stderr);
}

void installFontMessageFilter()
{
    if (messageFilterInstalled)
        return;
    previousMessageHandler = qInstallMessageHandler(fontMessageHandler);
    messageFilterInstalled = true;
}

} // namespace

extern "C" bool qbz_register_devanagari_fallback()
{
    // This is an application font, not the UI font.  Register it only for
    // Devanagari so Qt never has to walk (and warn about) unrelated system
    // families when catalog metadata contains that script.
    const auto id = QFontDatabase::addApplicationFont(QStringLiteral(
        ":/qt/qml/com/blitzfc/qbz/qml/assets/fonts/"
        "NotoSansDevanagari-VariableFont_wght.ttf"));
    if (id < 0)
        return false;

    const auto families = QFontDatabase::applicationFontFamilies(id);
    if (families.isEmpty())
        return false;
    QFontDatabase::addApplicationFallbackFontFamily(
        QChar::Script_Devanagari, families.constFirst());
    installFontMessageFilter();
    return true;
}

extern "C" bool qbz_register_japanese_fallback()
{
    // LINE Seed JP is already bundled as a selectable UI/lyrics face.  Make
    // it Qt's deterministic fallback for the three scripts used by Japanese
    // text as well.  Without this, a Latin-only choice such as Source Sans 3
    // delegates to the host's first fontconfig match; a user-installed
    // MS Gothic can therefore receive synthetic Medium/Bold and render much
    // heavier than the surrounding UI.
    const auto id = QFontDatabase::addApplicationFont(QStringLiteral(
        ":/qt/qml/com/blitzfc/qbz/qml/assets/fonts/"
        "LINESeedJP-Regular.ttf"));
    if (id < 0)
        return false;

    const auto families = QFontDatabase::applicationFontFamilies(id);
    if (families.isEmpty())
        return false;
    const auto &family = families.constFirst();
    QFontDatabase::addApplicationFallbackFontFamily(QChar::Script_Han, family);
    QFontDatabase::addApplicationFallbackFontFamily(QChar::Script_Hiragana, family);
    QFontDatabase::addApplicationFallbackFontFamily(QChar::Script_Katakana, family);
    return true;
}
