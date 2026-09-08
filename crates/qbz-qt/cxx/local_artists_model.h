#ifndef QBZ_LOCAL_ARTISTS_MODEL_H
#define QBZ_LOCAL_ARTISTS_MODEL_H

#include <QtCore/QAbstractListModel>
#include <QtCore/QHash>
#include <QtCore/QList>
#include <QtCore/QVariantMap>

class LocalArtistsModel final : public QAbstractListModel
{
    Q_OBJECT
    Q_PROPERTY(int generation READ generation NOTIFY generationChanged)
    Q_PROPERTY(qint64 totalCount READ totalCount NOTIFY totalCountChanged)
    Q_PROPERTY(qint64 artistTotal READ artistTotal NOTIFY artistTotalChanged)
    Q_PROPERTY(int residentArtists READ residentArtists NOTIFY residentArtistsChanged)
    Q_PROPERTY(qint64 evictions READ evictions NOTIFY evictionsChanged)

public:
    enum Role { ModelDataRole = Qt::UserRole + 1 };

    explicit LocalArtistsModel(QObject *parent = nullptr);

    int rowCount(const QModelIndex &parent = QModelIndex()) const override;
    QVariant data(const QModelIndex &index, int role = Qt::DisplayRole) const override;
    QHash<int, QByteArray> roleNames() const override;

    int generation() const { return m_generation; }
    qint64 totalCount() const { return m_totalCount; }
    qint64 artistTotal() const { return m_artistTotal; }
    int residentArtists() const { return m_residentArtists; }
    qint64 evictions() const { return m_evictions; }

    Q_INVOKABLE QVariantMap rowAt(qint64 row) const;
    Q_INVOKABLE void setArtwork(const QString &artKey, const QString &path);

    void resetQuery(int generation, qint64 totalCount, qint64 artistTotal);
    bool applyPage(int generation, int page, const QByteArray &json);

signals:
    void generationChanged();
    void totalCountChanged();
    void artistTotalChanged();
    void residentArtistsChanged();
    void evictionsChanged();
    void pageMiss(int page, int generation);

private:
    struct Entry {
        QVariantMap values;
        QString artKey;
        bool isArtist = false;
    };
    using Page = QList<Entry>;

    static constexpr int PageEntries = 100;
    static constexpr int MaxResidentArtists = 2000;

    QVariantMap modelData(qint64 row) const;
    void requestPage(int page, bool prefetch = false) const;
    void touch(int page) const;
    void evictIfNeeded();

    int m_generation = 0;
    qint64 m_totalCount = 0;
    qint64 m_artistTotal = 0;
    QHash<int, Page> m_pages;
    mutable QList<int> m_lru;
    mutable QHash<int, bool> m_pending;
    int m_residentArtists = 0;
    qint64 m_evictions = 0;
};

extern "C" {
void qbz_local_artists_register_qml_type();
void qbz_local_artists_reset(int generation, qint64 totalCount, qint64 artistTotal);
bool qbz_local_artists_apply_page(int generation, int page, const char *json);
}

#endif // QBZ_LOCAL_ARTISTS_MODEL_H
