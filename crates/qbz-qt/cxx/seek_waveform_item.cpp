#include "seek_waveform_item.h"

#include <algorithm>
#include <atomic>
#include <cmath>
#include <limits>
#include <vector>

#include <QtQml/qqml.h>
#include <QtQuick/QSGClipNode>
#include <QtQuick/QSGGeometryNode>
#include <QtQuick/QSGVertexColorMaterial>

namespace {

QSGGeometryNode *makeLayer()
{
    auto *node = new QSGGeometryNode;
    auto *geometry = new QSGGeometry(QSGGeometry::defaultAttributes_ColoredPoint2D(), 0);
    geometry->setDrawingMode(QSGGeometry::DrawTriangles);
    node->setGeometry(geometry);
    node->setFlag(QSGNode::OwnsGeometry);
    node->setMaterial(new QSGVertexColorMaterial);
    node->setFlag(QSGNode::OwnsMaterial);
    return node;
}

class WaveformRootNode : public QSGNode
{
public:
    WaveformRootNode()
    {
        baseClip.setIsRectangular(true);
        cacheClip.setIsRectangular(true);
        playedClip.setIsRectangular(true);
        baseLayer = makeLayer();
        cacheLayer = makeLayer();
        playedLayer = makeLayer();
        baseClip.appendChildNode(baseLayer);
        cacheClip.appendChildNode(cacheLayer);
        playedClip.appendChildNode(playedLayer);
        appendChildNode(&baseClip);
        appendChildNode(&cacheClip);
        appendChildNode(&playedClip);
    }

    ~WaveformRootNode() override
    {
        removeChildNode(&baseClip);
        removeChildNode(&cacheClip);
        removeChildNode(&playedClip);
    }

    QSGClipNode baseClip;
    QSGClipNode cacheClip;
    QSGClipNode playedClip;
    QSGGeometryNode *baseLayer = nullptr;
    QSGGeometryNode *cacheLayer = nullptr;
    QSGGeometryNode *playedLayer = nullptr;
    quint64 geometryRevision = std::numeric_limits<quint64>::max();
    QSizeF geometrySize;
    QColor baseGeometryColor;
    QColor cacheGeometryColor;
    QColor playedGeometryColor;
    int geometryRenderMode = -1;
};

void setVertex(QSGGeometry::ColoredPoint2D &vertex,
               float x,
               float y,
               const QColor &color,
               float alpha)
{
    const float combined = std::clamp(static_cast<float>(color.alphaF()) * alpha, 0.0f, 1.0f);
    vertex.set(x,
               y,
               static_cast<unsigned char>(std::round(color.red() * combined)),
               static_cast<unsigned char>(std::round(color.green() * combined)),
               static_cast<unsigned char>(std::round(color.blue() * combined)),
               static_cast<unsigned char>(std::round(255.0f * combined)));
}

void setQuad(QSGGeometry::ColoredPoint2D *vertices,
             float left,
             float top,
             float right,
             float bottom,
             const QColor &color,
             float topAlpha,
             float bottomAlpha)
{
    setVertex(vertices[0], left, top, color, topAlpha);
    setVertex(vertices[1], left, bottom, color, bottomAlpha);
    setVertex(vertices[2], right, top, color, topAlpha);
    setVertex(vertices[3], right, top, color, topAlpha);
    setVertex(vertices[4], left, bottom, color, bottomAlpha);
    setVertex(vertices[5], right, bottom, color, bottomAlpha);
}

constexpr int capsuleCapSegments = 4;
constexpr int capsuleVertexCount = 12 + capsuleCapSegments * 6;

void setTriangle(QSGGeometry::ColoredPoint2D *vertices,
                 const QPointF &first,
                 const QPointF &second,
                 const QPointF &third,
                 const QColor &color,
                 float firstAlpha,
                 float secondAlpha,
                 float thirdAlpha)
{
    setVertex(vertices[0], static_cast<float>(first.x()), static_cast<float>(first.y()),
              color, firstAlpha);
    setVertex(vertices[1], static_cast<float>(second.x()), static_cast<float>(second.y()),
              color, secondAlpha);
    setVertex(vertices[2], static_cast<float>(third.x()), static_cast<float>(third.y()),
              color, thirdAlpha);
}

void setVerticalCapsule(QSGGeometry::ColoredPoint2D *vertices,
                        float left,
                        float top,
                        float right,
                        float bottom,
                        const QColor &color)
{
    constexpr qreal pi = 3.14159265358979323846;
    const float centreX = (left + right) * 0.5f;
    const float centreY = (top + bottom) * 0.5f;
    const float radius = std::min((right - left) * 0.5f, (bottom - top) * 0.5f);
    const float topCentre = top + radius;
    const float bottomCentre = bottom - radius;

    setQuad(vertices, left, topCentre, right, centreY, color, 0.62f, 1.0f);
    setQuad(vertices + 6, left, centreY, right, bottomCentre, color, 1.0f, 0.58f);

    int vertex = 12;
    for (int segment = 0; segment < capsuleCapSegments; ++segment) {
        const qreal firstAngle = pi + pi * segment / capsuleCapSegments;
        const qreal secondAngle = pi + pi * (segment + 1) / capsuleCapSegments;
        setTriangle(vertices + vertex,
                    QPointF(centreX, topCentre),
                    QPointF(centreX + std::cos(firstAngle) * radius,
                            topCentre + std::sin(firstAngle) * radius),
                    QPointF(centreX + std::cos(secondAngle) * radius,
                            topCentre + std::sin(secondAngle) * radius),
                    color, 0.62f, 0.42f, 0.42f);
        vertex += 3;
    }
    for (int segment = 0; segment < capsuleCapSegments; ++segment) {
        const qreal firstAngle = pi * segment / capsuleCapSegments;
        const qreal secondAngle = pi * (segment + 1) / capsuleCapSegments;
        setTriangle(vertices + vertex,
                    QPointF(centreX, bottomCentre),
                    QPointF(centreX + std::cos(firstAngle) * radius,
                            bottomCentre + std::sin(firstAngle) * radius),
                    QPointF(centreX + std::cos(secondAngle) * radius,
                            bottomCentre + std::sin(secondAngle) * radius),
                    color, 0.58f, 0.38f, 0.38f);
        vertex += 3;
    }
}

void setLayerGeometry(QSGGeometryNode *node,
                      const std::vector<float> &values,
                      qreal width,
                      qreal height,
                      const QColor &color,
                      int renderMode)
{
    auto *geometry = node->geometry();
    const int count = static_cast<int>(values.size());
    const float centre = static_cast<float>(height * 0.5);
    const float halfHeight = std::max(1.0f, static_cast<float>(height * 0.47));

    if (renderMode == 1) {
        // Small NPB: this is a stable three-sine motif rather than decoded
        // RMS. At thirteen pixels the real envelope cannot communicate track
        // structure, while a smooth repeated contour remains readable and
        // still carries played/cache clipping as a seek affordance.
        geometry->allocate(6 + std::max(0, count - 1) * 6);
        geometry->setDrawingMode(QSGGeometry::DrawTriangles);
        auto *vertices = geometry->vertexDataAsColoredPoint2D();
        const float railHalf = std::min(0.42f, halfHeight * 0.12f);
        setQuad(vertices, 0.0f, centre - railHalf, static_cast<float>(width),
                centre + railHalf, color, 0.32f, 0.32f);
        if (count < 2) {
            node->markDirty(QSGNode::DirtyGeometry);
            return;
        }

        const float lineHalf = std::clamp(static_cast<float>(height * 0.085), 0.62f, 1.15f);
        const float swing = static_cast<float>(height * 0.36);
        auto pointAt = [&](int index) {
            const float x = index * static_cast<float>(width) / (count - 1);
            const float y = centre + (0.5f - values[index]) * swing * 2.0f;
            return QPointF(x, y);
        };
        int vertex = 6;
        for (int index = 0; index + 1 < count; ++index) {
            const QPointF first = pointAt(index);
            const QPointF second = pointAt(index + 1);
            const QPointF delta = second - first;
            const qreal length = std::hypot(delta.x(), delta.y());
            const QPointF normal = length > 0.0001
                ? QPointF(-delta.y() / length * lineHalf,
                          delta.x() / length * lineHalf)
                : QPointF(0.0, lineHalf);
            setVertex(vertices[vertex], static_cast<float>((first + normal).x()),
                      static_cast<float>((first + normal).y()), color, 0.92f);
            setVertex(vertices[vertex + 1], static_cast<float>((first - normal).x()),
                      static_cast<float>((first - normal).y()), color, 0.92f);
            setVertex(vertices[vertex + 2], static_cast<float>((second + normal).x()),
                      static_cast<float>((second + normal).y()), color, 0.92f);
            setVertex(vertices[vertex + 3], static_cast<float>((second + normal).x()),
                      static_cast<float>((second + normal).y()), color, 0.92f);
            setVertex(vertices[vertex + 4], static_cast<float>((first - normal).x()),
                      static_cast<float>((first - normal).y()), color, 0.92f);
            setVertex(vertices[vertex + 5], static_cast<float>((second - normal).x()),
                      static_cast<float>((second - normal).y()), color, 0.92f);
            vertex += 6;
        }
        node->markDirty(QSGNode::DirtyGeometry);
        return;
    }

    const int visibleBars = static_cast<int>(std::count_if(values.begin(), values.end(),
        [](float value) { return value > 0.0005f; }));
    geometry->allocate(6 + visibleBars * capsuleVertexCount);
    geometry->setDrawingMode(QSGGeometry::DrawTriangles);
    auto *vertices = geometry->vertexDataAsColoredPoint2D();

    // A permanent hairline makes the temporal axis readable before the whole
    // track has been analysed. Its three clipped color layers retain the
    // existing played/cache/base semantics.
    const float railHalf = std::min(0.55f, halfHeight * 0.16f);
    setQuad(vertices, 0.0f, centre - railHalf, static_cast<float>(width), centre + railHalf,
            color, 0.46f, 0.46f);

    if (count == 0) {
        node->markDirty(QSGNode::DirtyGeometry);
        return;
    }

    constexpr int groupSize = 12;
    const int groupBreaks = (count - 1) / groupSize;
    const float nominalSlot = static_cast<float>(width) / count;
    const float groupGap = std::min(2.4f, std::max(0.9f, nominalSlot * 0.34f));
    const float usableWidth = std::max(1.0f, static_cast<float>(width) - groupBreaks * groupGap);
    const float slot = usableWidth / count;
    const float barWidth = std::clamp(slot * 0.56f, 1.8f, 3.6f);
    int vertex = 6;

    for (int index = 0; index < count; ++index) {
        const float value = std::clamp(values[index], 0.0f, 1.0f);
        if (value <= 0.0005f)
            continue;
        const float amplitude = std::min(halfHeight,
            0.85f + value * std::max(0.0f, halfHeight - 0.85f));
        const float left = slot * index + groupGap * (index / groupSize)
            + (slot - barWidth) * 0.5f;
        const float right = left + barWidth;
        setVerticalCapsule(vertices + vertex, left, centre - amplitude,
                           right, centre + amplitude, color);
        vertex += capsuleVertexCount;
    }
    node->markDirty(QSGNode::DirtyGeometry);
}

std::vector<float> renderValues(const QVariantList &source, int columns, float gamma)
{
    if (source.isEmpty() || columns < 2)
        return {};

    std::vector<float> values(columns, 0.0f);
    const int sourceCount = source.size();
    for (int column = 0; column < columns; ++column) {
        const int begin = column * sourceCount / columns;
        const int end = std::max(begin + 1, (column + 1) * sourceCount / columns);
        float peak = 0.0f;
        for (int index = begin; index < std::min(end, sourceCount); ++index)
            peak = std::max(peak, std::clamp(source[index].toFloat(), 0.0f, 1.0f));
        values[column] = std::pow(peak, gamma);
    }

    std::vector<float> scratch(columns, 0.0f);
    for (int pass = 0; pass < 2; ++pass) {
        scratch.front() = values.front();
        scratch.back() = values.back();
        for (int index = 1; index + 1 < columns; ++index) {
            scratch[index] = values[index - 1] * 0.20f
                + values[index] * 0.60f
                + values[index + 1] * 0.20f;
        }
        values.swap(scratch);
    }
    const float maximum = *std::max_element(values.begin(), values.end());
    if (maximum > 0.0005f) {
        for (float &value : values)
            value = std::clamp(value / maximum, 0.0f, 1.0f);
    }
    return values;
}

std::vector<float> decorativeWaveValues(int columns, qreal width)
{
    std::vector<float> values(columns, 0.5f);
    if (columns < 2)
        return values;

    constexpr qreal twoPi = 6.28318530717958647692;
    const qreal repeats = std::max<qreal>(3.0, width / 210.0);
    for (int index = 0; index < columns; ++index) {
        const qreal progress = static_cast<qreal>(index) / (columns - 1);
        const qreal phase = twoPi * repeats * progress;
        const qreal composite = 0.55 * std::sin(phase)
            + 0.28 * std::sin(phase * 2.0 + 0.85)
            + 0.17 * std::sin(phase * 3.0 + 2.1);
        values[index] = static_cast<float>(0.5 + composite * 0.46);
    }
    return values;
}

} // namespace

SeekWaveformItem::SeekWaveformItem(QQuickItem *parent)
    : QQuickItem(parent)
{
    setFlag(ItemHasContents, true);
    setAntialiasing(true);
}

void SeekWaveformItem::setValues(const QVariantList &values)
{
    if (m_values == values)
        return;
    m_values = values;
    ++m_valuesRevision;
    update();
}

void SeekWaveformItem::setPlayedProgress(qreal progress)
{
    progress = qBound<qreal>(0.0, progress, 1.0);
    if (qFuzzyCompare(m_playedProgress, progress))
        return;
    m_playedProgress = progress;
    update();
}

void SeekWaveformItem::setCacheProgress(qreal progress)
{
    progress = qBound<qreal>(0.0, progress, 1.0);
    if (qFuzzyCompare(m_cacheProgress, progress))
        return;
    m_cacheProgress = progress;
    update();
}

void SeekWaveformItem::setBaseColor(const QColor &color)
{
    if (m_baseColor == color)
        return;
    m_baseColor = color;
    update();
}

void SeekWaveformItem::setCacheColor(const QColor &color)
{
    if (m_cacheColor == color)
        return;
    m_cacheColor = color;
    update();
}

void SeekWaveformItem::setPlayedColor(const QColor &color)
{
    if (m_playedColor == color)
        return;
    m_playedColor = color;
    update();
}

void SeekWaveformItem::setRenderMode(int mode)
{
    mode = mode == 1 ? 1 : 0;
    if (m_renderMode == mode)
        return;
    m_renderMode = mode;
    update();
}

QSGNode *SeekWaveformItem::updatePaintNode(QSGNode *oldNode, UpdatePaintNodeData *)
{
    if ((m_values.isEmpty() && m_renderMode == 0) || width() <= 0.0 || height() <= 0.0) {
        delete oldNode;
        return nullptr;
    }

    auto *root = static_cast<WaveformRootNode *>(oldNode);
    if (!root)
        root = new WaveformRootNode;

    const QRectF fullRect(0.0, 0.0, width(), height());
    root->baseClip.setClipRect(fullRect);
    root->cacheClip.setClipRect(QRectF(0.0, 0.0, width() * m_cacheProgress, height()));
    root->playedClip.setClipRect(QRectF(0.0, 0.0, width() * m_playedProgress, height()));

    const bool colorsChanged = root->baseGeometryColor != m_baseColor
        || root->cacheGeometryColor != m_cacheColor
        || root->playedGeometryColor != m_playedColor;
    if (root->geometryRevision != m_valuesRevision || root->geometrySize != size()
        || colorsChanged || root->geometryRenderMode != m_renderMode) {
        const int columns = m_renderMode == 1
            ? qBound(64, static_cast<int>(std::ceil(width() / 3.0)), 512)
            : qBound(48, static_cast<int>(std::ceil(width() / 7.5)), 240);
        const auto values = m_renderMode == 1
            ? decorativeWaveValues(columns, width())
            : renderValues(m_values, columns, 0.72f);
        setLayerGeometry(root->baseLayer, values, width(), height(), m_baseColor, m_renderMode);
        setLayerGeometry(root->cacheLayer, values, width(), height(), m_cacheColor, m_renderMode);
        setLayerGeometry(root->playedLayer, values, width(), height(), m_playedColor, m_renderMode);
        root->geometryRevision = m_valuesRevision;
        root->geometrySize = size();
        root->baseGeometryColor = m_baseColor;
        root->cacheGeometryColor = m_cacheColor;
        root->playedGeometryColor = m_playedColor;
        root->geometryRenderMode = m_renderMode;
    }
    return root;
}

extern "C" void qbz_seek_waveform_register_qml_type()
{
    static std::atomic_bool registered{ false };
    if (registered.exchange(true))
        return;
    qmlRegisterType<SeekWaveformItem>("com.blitzfc.qbz", 1, 0, "SeekWaveformItem");
}
