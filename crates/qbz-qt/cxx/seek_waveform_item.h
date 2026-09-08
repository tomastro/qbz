#ifndef QBZ_SEEK_WAVEFORM_ITEM_H
#define QBZ_SEEK_WAVEFORM_ITEM_H

#include <QtCore/QVariantList>
#include <QtGui/QColor>
#include <QtQuick/QQuickItem>

class SeekWaveformItem : public QQuickItem
{
    Q_OBJECT
    Q_PROPERTY(QVariantList values READ values WRITE setValues)
    Q_PROPERTY(qreal playedProgress READ playedProgress WRITE setPlayedProgress)
    Q_PROPERTY(qreal cacheProgress READ cacheProgress WRITE setCacheProgress)
    Q_PROPERTY(QColor baseColor READ baseColor WRITE setBaseColor)
    Q_PROPERTY(QColor cacheColor READ cacheColor WRITE setCacheColor)
    Q_PROPERTY(QColor playedColor READ playedColor WRITE setPlayedColor)
    Q_PROPERTY(int renderMode READ renderMode WRITE setRenderMode)

public:
    explicit SeekWaveformItem(QQuickItem *parent = nullptr);

    QVariantList values() const { return m_values; }
    void setValues(const QVariantList &values);
    qreal playedProgress() const { return m_playedProgress; }
    void setPlayedProgress(qreal progress);
    qreal cacheProgress() const { return m_cacheProgress; }
    void setCacheProgress(qreal progress);
    QColor baseColor() const { return m_baseColor; }
    void setBaseColor(const QColor &color);
    QColor cacheColor() const { return m_cacheColor; }
    void setCacheColor(const QColor &color);
    QColor playedColor() const { return m_playedColor; }
    void setPlayedColor(const QColor &color);
    int renderMode() const { return m_renderMode; }
    void setRenderMode(int mode);

protected:
    QSGNode *updatePaintNode(QSGNode *oldNode, UpdatePaintNodeData *) override;

private:
    QVariantList m_values;
    qreal m_playedProgress = 0.0;
    qreal m_cacheProgress = 0.0;
    QColor m_baseColor{ 64, 68, 74 };
    QColor m_cacheColor{ 128, 132, 140 };
    QColor m_playedColor{ 63, 169, 232 };
    int m_renderMode = 0;
    quint64 m_valuesRevision = 0;
};

extern "C" void qbz_seek_waveform_register_qml_type();

#endif
