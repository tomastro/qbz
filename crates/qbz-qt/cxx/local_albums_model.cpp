#include "local_albums_model.h"

#include <QtCore/QCoreApplication>
#include <QtCore/QJsonArray>
#include <QtCore/QJsonDocument>
#include <QtCore/QJsonObject>
#include <QtCore/QMetaMethod>
#include <QtCore/QMetaObject>
#include <QtCore/QPointer>
#include <QtQml/qqml.h>

#include <limits>

namespace {
QPointer<LocalAlbumsModel> g_model;
QPointer<LocalAlbumsModel> g_artist_model;

void registerLocalAlbumsModel()
{
    auto *application = QCoreApplication::instance();
    auto *model = new LocalAlbumsModel(application);
    g_model = model;
    qmlRegisterSingletonInstance<LocalAlbumsModel>(
        "com.blitzfc.qbz", 1, 0, "QbzLocalAlbums", model);
    auto *artistModel = new LocalAlbumsModel(application);
    g_artist_model = artistModel;
    qmlRegisterSingletonInstance<LocalAlbumsModel>(
        "com.blitzfc.qbz", 1, 0, "QbzLocalArtistAlbums", artistModel);
}

Q_COREAPP_STARTUP_FUNCTION(registerLocalAlbumsModel)
}

LocalAlbumsModel::LocalAlbumsModel(QObject *parent)
    : QAbstractListModel(parent)
{
}

int LocalAlbumsModel::rowCount(const QModelIndex &parent) const
{
    if (parent.isValid())
        return 0;
    return int(qMin<qint64>(m_totalCount, std::numeric_limits<int>::max()));
}

QVariant LocalAlbumsModel::data(const QModelIndex &index, int role) const
{
    if (!index.isValid() || role != ModelDataRole)
        return {};
    return modelData(index.row());
}

QHash<int, QByteArray> LocalAlbumsModel::roleNames() const
{
    return {{ ModelDataRole, QByteArrayLiteral("modelData") }};
}

QVariantMap LocalAlbumsModel::rowAt(qint64 row) const
{
    if (row < 0 || row >= m_totalCount)
        return {};
    return modelData(row);
}

QVariantMap LocalAlbumsModel::modelData(qint64 row) const
{
    const int page = int(row / PageEntries);
    const int offset = int(row % PageEntries);
    const auto found = m_pages.constFind(page);
    if (found == m_pages.cend() || offset >= found->size()) {
        requestPage(page, false);
        return {
            { QStringLiteral("t"), 1 },
            { QStringLiteral("loading"), true },
            { QStringLiteral("base"), 0 },
            { QStringLiteral("items"), QVariantList() },
        };
    }
    touch(page);
    QVariantMap values = found->at(offset).values;
    values.insert(QStringLiteral("loading"), false);
    QVariantList items = values.value(QStringLiteral("items")).toList();
    for (QVariant &value : items) {
        QVariantMap album = value.toMap();
        album.insert(
            QStringLiteral("selected"),
            selected(album.value(QStringLiteral("nativeIndex")).toLongLong()));
        value = album;
    }
    values.insert(QStringLiteral("items"), items);
    return values;
}

void LocalAlbumsModel::requestPage(int page, bool prefetch) const
{
    if (page < 0)
        return;
    const QMetaMethod missSignal = QMetaMethod::fromSignal(&LocalAlbumsModel::pageMiss);
    if (!isSignalConnected(missSignal))
        return;
    if (m_pending.contains(page)) {
        if (!prefetch)
            m_pending.insert(page, false);
        return;
    }
    m_pending.insert(page, prefetch);
    const int generation = m_generation;
    auto *self = const_cast<LocalAlbumsModel *>(this);
    QMetaObject::invokeMethod(
        self,
        [self, page, generation]() {
            if (generation != self->m_generation)
                return;
            const QMetaMethod missSignal =
                QMetaMethod::fromSignal(&LocalAlbumsModel::pageMiss);
            if (self->isSignalConnected(missSignal)) {
                emit self->pageMiss(page, generation);
            } else {
                self->m_pending.remove(page);
            }
        },
        Qt::QueuedConnection);
}

void LocalAlbumsModel::touch(int page) const
{
    m_lru.removeAll(page);
    m_lru.append(page);
}

bool LocalAlbumsModel::selected(qint64 row) const
{
    bool inRange = false;
    for (const auto &range : m_selectionRanges) {
        if (row < range.first)
            break;
        if (row <= range.second) {
            inRange = true;
            break;
        }
    }
    return m_selectAll ? !inRange : inRange;
}

void LocalAlbumsModel::resetQuery(int generation, qint64 totalCount, qint64 albumTotal)
{
    beginResetModel();
    m_generation = generation;
    m_totalCount = qMax<qint64>(0, totalCount);
    m_albumTotal = qMax<qint64>(0, albumTotal);
    m_pages.clear();
    m_lru.clear();
    m_pending.clear();
    m_residentAlbums = 0;
    m_evictions = 0;
    m_selectAll = false;
    m_selectionRanges.clear();
    m_selectedCount = 0;
    endResetModel();
    emit generationChanged();
    emit totalCountChanged();
    emit albumTotalChanged();
    emit residentAlbumsChanged();
    emit evictionsChanged();
    emit selectedCountChanged();
}

bool LocalAlbumsModel::applyPage(int generation, int page, const QByteArray &json)
{
    if (generation != m_generation || page < 0)
        return false;
    QJsonParseError error;
    const QJsonDocument document = QJsonDocument::fromJson(json, &error);
    if (error.error != QJsonParseError::NoError || !document.isArray())
        return false;

    Page entries;
    const QJsonArray array = document.array();
    entries.reserve(array.size());
    for (const QJsonValue &value : array) {
        if (!value.isObject())
            return false;
        Entry entry;
        entry.values = value.toObject().toVariantMap();
        const QVariantList items = entry.values.value(QStringLiteral("items")).toList();
        entry.albumCount = items.size();
        for (const QVariant &item : items) {
            const QString key = item.toMap().value(QStringLiteral("artKey")).toString();
            if (!key.isEmpty())
                entry.artKeys.append(key);
        }
        entries.append(std::move(entry));
    }

    const auto previous = m_pages.constFind(page);
    if (previous != m_pages.cend()) {
        for (const Entry &entry : *previous)
            m_residentAlbums -= entry.albumCount;
    }
    m_pages.insert(page, std::move(entries));
    for (const Entry &entry : m_pages.value(page))
        m_residentAlbums += entry.albumCount;
    const bool wasPrefetch = m_pending.take(page);
    touch(page);
    evictIfNeeded();

    const int first = page * PageEntries;
    const int last = qMin(rowCount() - 1, first + PageEntries - 1);
    if (first <= last)
        emit dataChanged(index(first, 0), index(last, 0), { ModelDataRole });
    emit residentAlbumsChanged();

    const int next = page + 1;
    if (!wasPrefetch && qint64(next) * PageEntries < m_totalCount && !m_pages.contains(next))
        requestPage(next, true);
    return true;
}

void LocalAlbumsModel::evictIfNeeded()
{
    while (m_residentAlbums > MaxResidentAlbums && m_pages.size() > 1 && !m_lru.isEmpty()) {
        const int page = m_lru.takeFirst();
        const auto found = m_pages.find(page);
        if (found == m_pages.end())
            continue;
        for (const Entry &entry : *found)
            m_residentAlbums -= entry.albumCount;
        m_pages.erase(found);
        ++m_evictions;
        const int first = page * PageEntries;
        const int last = qMin(rowCount() - 1, first + PageEntries - 1);
        if (first <= last)
            emit dataChanged(index(first, 0), index(last, 0), { ModelDataRole });
    }
    emit evictionsChanged();
}

void LocalAlbumsModel::setArtwork(const QString &artKey, const QString &path)
{
    if (artKey.isEmpty() || path.isEmpty())
        return;
    for (auto page = m_pages.begin(); page != m_pages.end(); ++page) {
        for (int offset = 0; offset < page->size(); ++offset) {
            Entry &entry = (*page)[offset];
            if (!entry.artKeys.contains(artKey))
                continue;
            QVariantList items = entry.values.value(QStringLiteral("items")).toList();
            for (QVariant &value : items) {
                QVariantMap album = value.toMap();
                if (album.value(QStringLiteral("artKey")).toString() == artKey)
                    album.insert(QStringLiteral("artPath"), path);
                value = album;
            }
            entry.values.insert(QStringLiteral("items"), items);
            const int row = page.key() * PageEntries + offset;
            emit dataChanged(index(row, 0), index(row, 0), { ModelDataRole });
        }
    }
}

void LocalAlbumsModel::setSelection(
    int generation, const QByteArray &json, qint64 selectedCount)
{
    if (generation != m_generation)
        return;
    const QJsonDocument document = QJsonDocument::fromJson(json);
    if (!document.isObject())
        return;
    const QJsonObject object = document.object();
    QList<QPair<qint64, qint64>> ranges;
    for (const QJsonValue &value : object.value(QStringLiteral("ranges")).toArray()) {
        const QJsonArray pair = value.toArray();
        if (pair.size() != 2)
            continue;
        ranges.append({ qMax<qint64>(0, pair.at(0).toInteger()),
                        qMax<qint64>(0, pair.at(1).toInteger()) });
    }
    m_selectAll = object.value(QStringLiteral("all")).toBool();
    m_selectionRanges = std::move(ranges);
    m_selectedCount = qMax<qint64>(0, selectedCount);
    emit selectedCountChanged();
    if (rowCount() > 0)
        emit dataChanged(index(0, 0), index(rowCount() - 1, 0), { ModelDataRole });
}

extern "C" void qbz_local_albums_register_qml_type()
{
    // Link anchor. Registration is performed by Q_COREAPP_STARTUP_FUNCTION.
}

extern "C" void qbz_local_albums_reset(
    int generation, qint64 totalCount, qint64 albumTotal)
{
    if (g_model)
        g_model->resetQuery(generation, totalCount, albumTotal);
}

extern "C" bool qbz_local_albums_apply_page(int generation, int page, const char *json)
{
    return g_model && g_model->applyPage(
        generation, page, QByteArray(json ? json : "[]"));
}

extern "C" void qbz_local_albums_set_selection(
    int generation, const char *json, qint64 selectedCount)
{
    if (g_model)
        g_model->setSelection(
            generation, QByteArray(json ? json : "{}"), selectedCount);
}

extern "C" void qbz_local_artist_albums_reset(
    int generation, qint64 totalCount, qint64 albumTotal)
{
    if (g_artist_model)
        g_artist_model->resetQuery(generation, totalCount, albumTotal);
}

extern "C" bool qbz_local_artist_albums_apply_page(
    int generation, int page, const char *json)
{
    return g_artist_model && g_artist_model->applyPage(
        generation, page, QByteArray(json ? json : "[]"));
}
