#include "local_tracks_model.h"

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
QPointer<LocalTracksModel> g_model;

void registerLocalTracksModel()
{
    auto *application = QCoreApplication::instance();
    auto *model = new LocalTracksModel(application);
    g_model = model;
    qmlRegisterSingletonInstance<LocalTracksModel>(
        "com.blitzfc.qbz", 1, 0, "QbzLocalTracks", model);
}

Q_COREAPP_STARTUP_FUNCTION(registerLocalTracksModel)
}

LocalTracksModel::LocalTracksModel(QObject *parent)
    : QAbstractListModel(parent)
{
}

int LocalTracksModel::rowCount(const QModelIndex &parent) const
{
    if (parent.isValid())
        return 0;
    return int(qMin<qint64>(m_totalCount, std::numeric_limits<int>::max()));
}

QVariant LocalTracksModel::data(const QModelIndex &index, int role) const
{
    if (!index.isValid() || role != ModelDataRole)
        return {};
    return modelData(index.row());
}

QHash<int, QByteArray> LocalTracksModel::roleNames() const
{
    return {{ ModelDataRole, QByteArrayLiteral("modelData") }};
}

QVariantMap LocalTracksModel::rowAt(qint64 row) const
{
    if (row < 0 || row >= m_totalCount)
        return {};
    return modelData(row);
}

QVariantMap LocalTracksModel::modelData(qint64 row) const
{
    const int page = int(row / PageRows);
    const int offset = int(row % PageRows);
    const auto found = m_pages.constFind(page);
    if (found == m_pages.cend() || offset >= found->size()) {
        requestPage(page, false);
        return {
            { QStringLiteral("t"), 1 },
            { QStringLiteral("n"), row + 1 },
            { QStringLiteral("loading"), true },
            { QStringLiteral("groupStart"), false },
            { QStringLiteral("groupLabel"), QString() },
            { QStringLiteral("selected"), false },
            { QStringLiteral("row"), QVariantMap() },
        };
    }
    touch(page);
    const TrackRow &track = found->at(offset);
    QVariantMap values = track.row;
    values.insert(QStringLiteral("selected"), selected(row));
    return {
        { QStringLiteral("t"), 1 },
        { QStringLiteral("n"), row + 1 },
        { QStringLiteral("loading"), false },
        { QStringLiteral("groupStart"), track.groupStart },
        { QStringLiteral("groupLabel"), track.groupLabel },
        { QStringLiteral("selected"), selected(row) },
        { QStringLiteral("row"), values },
    };
}

void LocalTracksModel::requestPage(int page, bool prefetch) const
{
    if (page < 0)
        return;
    const QMetaMethod missSignal = QMetaMethod::fromSignal(&LocalTracksModel::pageMiss);
    // The Local Library view is lazy. A prefetch published before its
    // Connections object exists would otherwise be lost while leaving this
    // page permanently marked pending across the later mount.
    if (!isSignalConnected(missSignal))
        return;
    if (m_pending.contains(page)) {
        if (!prefetch)
            m_pending.insert(page, false);
        return;
    }
    m_pending.insert(page, prefetch);
    const int generation = m_generation;
    auto *self = const_cast<LocalTracksModel *>(this);
    QMetaObject::invokeMethod(
        self,
        [self, page, generation]() {
            if (generation != self->m_generation)
                return;
            const QMetaMethod missSignal =
                QMetaMethod::fromSignal(&LocalTracksModel::pageMiss);
            if (self->isSignalConnected(missSignal)) {
                emit self->pageMiss(page, generation);
            } else {
                self->m_pending.remove(page);
            }
        },
        Qt::QueuedConnection);
}

void LocalTracksModel::touch(int page) const
{
    m_lru.removeAll(page);
    m_lru.append(page);
}

bool LocalTracksModel::selected(qint64 row) const
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

void LocalTracksModel::resetQuery(int generation, qint64 totalCount, const QString &group)
{
    beginResetModel();
    m_generation = generation;
    m_totalCount = qMax<qint64>(0, totalCount);
    m_group = group;
    m_pages.clear();
    m_lru.clear();
    m_pending.clear();
    m_residentRows = 0;
    m_evictions = 0;
    m_selectAll = false;
    m_selectionRanges.clear();
    m_selectedCount = 0;
    endResetModel();
    emit generationChanged();
    emit totalCountChanged();
    emit residentRowsChanged();
    emit evictionsChanged();
    emit selectedCountChanged();
}

bool LocalTracksModel::applyPage(int generation, int page, const QByteArray &json)
{
    if (generation != m_generation || page < 0)
        return false;
    QJsonParseError error;
    const QJsonDocument document = QJsonDocument::fromJson(json, &error);
    if (error.error != QJsonParseError::NoError || !document.isArray())
        return false;

    Page rows;
    const QJsonArray array = document.array();
    rows.reserve(array.size());
    for (const QJsonValue &value : array) {
        if (!value.isObject())
            return false;
        const QJsonObject object = value.toObject();
        TrackRow track;
        track.groupStart = object.value(QStringLiteral("groupStart")).toBool();
        track.groupLabel = object.value(QStringLiteral("groupLabel")).toString();
        track.row = object.value(QStringLiteral("row")).toObject().toVariantMap();
        track.artKey = track.row.value(QStringLiteral("artKey")).toString();
        rows.append(std::move(track));
    }

    const auto previous = m_pages.constFind(page);
    if (previous != m_pages.cend())
        m_residentRows -= previous->size();
    m_pages.insert(page, std::move(rows));
    m_residentRows += m_pages.value(page).size();
    const bool wasPrefetch = m_pending.take(page);
    touch(page);
    evictIfNeeded();

    const int first = page * PageRows;
    const int last = qMin(rowCount() - 1, first + PageRows - 1);
    if (first <= last)
        emit dataChanged(index(first, 0), index(last, 0), { ModelDataRole });
    emit residentRowsChanged();

    const int next = page + 1;
    if (!wasPrefetch && qint64(next) * PageRows < m_totalCount && !m_pages.contains(next))
        requestPage(next, true);
    return true;
}

void LocalTracksModel::evictIfNeeded()
{
    while (m_pages.size() > MaxResidentPages && !m_lru.isEmpty()) {
        const int page = m_lru.takeFirst();
        const auto found = m_pages.find(page);
        if (found == m_pages.end())
            continue;
        m_residentRows -= found->size();
        m_pages.erase(found);
        ++m_evictions;
        const int first = page * PageRows;
        const int last = qMin(rowCount() - 1, first + PageRows - 1);
        if (first <= last)
            emit dataChanged(index(first, 0), index(last, 0), { ModelDataRole });
    }
    emit evictionsChanged();
}

void LocalTracksModel::setArtwork(const QString &artKey, const QString &path)
{
    if (artKey.isEmpty() || path.isEmpty())
        return;
    for (auto page = m_pages.begin(); page != m_pages.end(); ++page) {
        for (int offset = 0; offset < page->size(); ++offset) {
            TrackRow &track = (*page)[offset];
            if (track.artKey != artKey)
                continue;
            track.row.insert(QStringLiteral("artPath"), path);
            const int row = page.key() * PageRows + offset;
            emit dataChanged(index(row, 0), index(row, 0), { ModelDataRole });
        }
    }
}

void LocalTracksModel::setSelection(
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

extern "C" void qbz_local_tracks_register_qml_type()
{
    // Link anchor. Registration is performed by Q_COREAPP_STARTUP_FUNCTION.
}

extern "C" void qbz_local_tracks_reset(
    int generation, qint64 totalCount, const char *group)
{
    if (g_model)
        g_model->resetQuery(generation, totalCount, QString::fromUtf8(group ? group : ""));
}

extern "C" bool qbz_local_tracks_apply_page(int generation, int page, const char *json)
{
    return g_model && g_model->applyPage(
        generation, page, QByteArray(json ? json : "[]"));
}

extern "C" void qbz_local_tracks_set_selection(
    int generation, const char *json, qint64 selectedCount)
{
    if (g_model)
        g_model->setSelection(
            generation, QByteArray(json ? json : "{}"), selectedCount);
}
