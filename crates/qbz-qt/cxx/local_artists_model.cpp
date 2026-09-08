#include "local_artists_model.h"

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
QPointer<LocalArtistsModel> g_model;

void registerLocalArtistsModel()
{
    auto *model = new LocalArtistsModel(QCoreApplication::instance());
    g_model = model;
    qmlRegisterSingletonInstance<LocalArtistsModel>(
        "com.blitzfc.qbz", 1, 0, "QbzLocalArtists", model);
}

Q_COREAPP_STARTUP_FUNCTION(registerLocalArtistsModel)
}

LocalArtistsModel::LocalArtistsModel(QObject *parent)
    : QAbstractListModel(parent)
{
}

int LocalArtistsModel::rowCount(const QModelIndex &parent) const
{
    if (parent.isValid())
        return 0;
    return int(qMin<qint64>(m_totalCount, std::numeric_limits<int>::max()));
}

QVariant LocalArtistsModel::data(const QModelIndex &index, int role) const
{
    if (!index.isValid() || role != ModelDataRole)
        return {};
    return modelData(index.row());
}

QHash<int, QByteArray> LocalArtistsModel::roleNames() const
{
    return {{ ModelDataRole, QByteArrayLiteral("modelData") }};
}

QVariantMap LocalArtistsModel::rowAt(qint64 row) const
{
    if (row < 0 || row >= m_totalCount)
        return {};
    return modelData(row);
}

QVariantMap LocalArtistsModel::modelData(qint64 row) const
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
            { QStringLiteral("item"), QVariantMap() },
        };
    }
    touch(page);
    QVariantMap values = found->at(offset).values;
    values.insert(QStringLiteral("loading"), false);
    return values;
}

void LocalArtistsModel::requestPage(int page, bool prefetch) const
{
    if (page < 0)
        return;
    const QMetaMethod missSignal = QMetaMethod::fromSignal(&LocalArtistsModel::pageMiss);
    if (!isSignalConnected(missSignal))
        return;
    if (m_pending.contains(page)) {
        if (!prefetch)
            m_pending.insert(page, false);
        return;
    }
    m_pending.insert(page, prefetch);
    const int generation = m_generation;
    auto *self = const_cast<LocalArtistsModel *>(this);
    QMetaObject::invokeMethod(
        self,
        [self, page, generation]() {
            if (generation != self->m_generation)
                return;
            if (self->isSignalConnected(
                    QMetaMethod::fromSignal(&LocalArtistsModel::pageMiss))) {
                emit self->pageMiss(page, generation);
            } else {
                self->m_pending.remove(page);
            }
        },
        Qt::QueuedConnection);
}

void LocalArtistsModel::touch(int page) const
{
    m_lru.removeAll(page);
    m_lru.append(page);
}

void LocalArtistsModel::resetQuery(int generation, qint64 totalCount, qint64 artistTotal)
{
    beginResetModel();
    m_generation = generation;
    m_totalCount = qMax<qint64>(0, totalCount);
    m_artistTotal = qMax<qint64>(0, artistTotal);
    m_pages.clear();
    m_lru.clear();
    m_pending.clear();
    m_residentArtists = 0;
    m_evictions = 0;
    endResetModel();
    emit generationChanged();
    emit totalCountChanged();
    emit artistTotalChanged();
    emit residentArtistsChanged();
    emit evictionsChanged();
}

bool LocalArtistsModel::applyPage(int generation, int page, const QByteArray &json)
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
        entry.isArtist = entry.values.value(QStringLiteral("t")).toInt() == 1;
        entry.artKey = entry.values.value(QStringLiteral("item"))
                           .toMap()
                           .value(QStringLiteral("artKey"))
                           .toString();
        entries.append(std::move(entry));
    }

    const auto previous = m_pages.constFind(page);
    if (previous != m_pages.cend()) {
        for (const Entry &entry : *previous)
            m_residentArtists -= int(entry.isArtist);
    }
    m_pages.insert(page, std::move(entries));
    for (const Entry &entry : m_pages.value(page))
        m_residentArtists += int(entry.isArtist);
    const bool wasPrefetch = m_pending.take(page);
    touch(page);
    evictIfNeeded();

    const int first = page * PageEntries;
    const int last = qMin(rowCount() - 1, first + PageEntries - 1);
    if (first <= last)
        emit dataChanged(index(first, 0), index(last, 0), { ModelDataRole });
    emit residentArtistsChanged();
    const int next = page + 1;
    if (!wasPrefetch && qint64(next) * PageEntries < m_totalCount && !m_pages.contains(next))
        requestPage(next, true);
    return true;
}

void LocalArtistsModel::evictIfNeeded()
{
    while (m_residentArtists > MaxResidentArtists && m_pages.size() > 1 && !m_lru.isEmpty()) {
        const int page = m_lru.takeFirst();
        const auto found = m_pages.find(page);
        if (found == m_pages.end())
            continue;
        for (const Entry &entry : *found)
            m_residentArtists -= int(entry.isArtist);
        m_pages.erase(found);
        ++m_evictions;
        const int first = page * PageEntries;
        const int last = qMin(rowCount() - 1, first + PageEntries - 1);
        if (first <= last)
            emit dataChanged(index(first, 0), index(last, 0), { ModelDataRole });
    }
    emit evictionsChanged();
}

void LocalArtistsModel::setArtwork(const QString &artKey, const QString &path)
{
    if (artKey.isEmpty() || path.isEmpty())
        return;
    for (auto page = m_pages.begin(); page != m_pages.end(); ++page) {
        for (int offset = 0; offset < page->size(); ++offset) {
            Entry &entry = (*page)[offset];
            if (entry.artKey != artKey)
                continue;
            QVariantMap item = entry.values.value(QStringLiteral("item")).toMap();
            item.insert(QStringLiteral("artPath"), path);
            entry.values.insert(QStringLiteral("item"), item);
            const int row = page.key() * PageEntries + offset;
            emit dataChanged(index(row, 0), index(row, 0), { ModelDataRole });
        }
    }
}

extern "C" void qbz_local_artists_register_qml_type()
{
    // Link anchor. Registration is performed by Q_COREAPP_STARTUP_FUNCTION.
}

extern "C" void qbz_local_artists_reset(
    int generation, qint64 totalCount, qint64 artistTotal)
{
    if (g_model)
        g_model->resetQuery(generation, totalCount, artistTotal);
}

extern "C" bool qbz_local_artists_apply_page(int generation, int page, const char *json)
{
    return g_model && g_model->applyPage(
        generation, page, QByteArray(json ? json : "[]"));
}
