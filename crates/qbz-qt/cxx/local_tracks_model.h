#ifndef QBZ_LOCAL_TRACKS_MODEL_H
#define QBZ_LOCAL_TRACKS_MODEL_H

#include <QtCore/QAbstractListModel>
#include <QtCore/QHash>
#include <QtCore/QList>
#include <QtCore/QVariantMap>

class LocalTracksModel final : public QAbstractListModel
{
    Q_OBJECT
    Q_PROPERTY(int generation READ generation NOTIFY generationChanged)
    Q_PROPERTY(qint64 totalCount READ totalCount NOTIFY totalCountChanged)
    Q_PROPERTY(int residentRows READ residentRows NOTIFY residentRowsChanged)
    Q_PROPERTY(qint64 evictions READ evictions NOTIFY evictionsChanged)
    Q_PROPERTY(qint64 selectedCount READ selectedCount NOTIFY selectedCountChanged)

public:
    enum Role { ModelDataRole = Qt::UserRole + 1 };

    explicit LocalTracksModel(QObject *parent = nullptr);

    int rowCount(const QModelIndex &parent = QModelIndex()) const override;
    QVariant data(const QModelIndex &index, int role = Qt::DisplayRole) const override;
    QHash<int, QByteArray> roleNames() const override;

    int generation() const { return m_generation; }
    qint64 totalCount() const { return m_totalCount; }
    int residentRows() const { return m_residentRows; }
    qint64 evictions() const { return m_evictions; }
    qint64 selectedCount() const { return m_selectedCount; }

    Q_INVOKABLE QVariantMap rowAt(qint64 row) const;
    Q_INVOKABLE void setArtwork(const QString &artKey, const QString &path);

    void resetQuery(int generation, qint64 totalCount, const QString &group);
    bool applyPage(int generation, int page, const QByteArray &json);
    void setSelection(int generation, const QByteArray &json, qint64 selectedCount);

signals:
    void generationChanged();
    void totalCountChanged();
    void residentRowsChanged();
    void evictionsChanged();
    void selectedCountChanged();
    void pageMiss(int page, int generation);

private:
    struct TrackRow {
        QVariantMap row;
        QString artKey;
        QString groupLabel;
        bool groupStart = false;
    };
    using Page = QList<TrackRow>;

    static constexpr int PageRows = 250;
    static constexpr int MaxResidentPages = 8;

    QVariantMap modelData(qint64 row) const;
    void requestPage(int page, bool prefetch = false) const;
    void touch(int page) const;
    bool selected(qint64 row) const;
    void evictIfNeeded();

    int m_generation = 0;
    qint64 m_totalCount = 0;
    QString m_group;
    QHash<int, Page> m_pages;
    mutable QList<int> m_lru;
    mutable QHash<int, bool> m_pending;
    int m_residentRows = 0;
    qint64 m_evictions = 0;
    bool m_selectAll = false;
    QList<QPair<qint64, qint64>> m_selectionRanges;
    qint64 m_selectedCount = 0;
};

extern "C" {
void qbz_local_tracks_register_qml_type();
void qbz_local_tracks_reset(int generation, qint64 totalCount, const char *group);
bool qbz_local_tracks_apply_page(int generation, int page, const char *json);
void qbz_local_tracks_set_selection(int generation, const char *json, qint64 selectedCount);
}

#endif // QBZ_LOCAL_TRACKS_MODEL_H
