// Spectral Ribbon (immersive shader scene, mode 4) — the QRhi renderer
// behind RibbonItem. Ports the spectrogram half of the Slint pipeline
// (crates/qbz/src/shader_underlay.rs:855-929): a persistent R8 texture
// (512 freq bands wide × 2048 time columns tall, SPECTRO_BANDS/COLS
// verbatim), one 512-byte row written at the playback-time column per
// frame, the SAME gap-fill (progress updates ~1 Hz, so the column jumps
// several slots — every skipped column gets the row), and a full clear on
// track change/seek (the reset flag). The shader is only a colorizer
// (spectral_ribbon.frag).
//
// Unlike the reference (which resets a thread-local LAST_COL on clear),
// the last-column state lives HERE, next to the texture it describes.
//
// Resource cadence (the pulse law): `synchronize` snapshots the frame
// QByteArray when the item updates (pulse tick only); `render` uploads the
// 512 B row (or the 1 MB clear, rare) and draws ONE fullscreen triangle.

#include "ribbon_item.h"

#include <QtCore/QCoreApplication>
#include <QtCore/QFile>
#include <QtQml/qqml.h>
#include <rhi/qrhi.h>

#include <cstring>

namespace {

// The lattice — pinned with spectral_ribbon.frag, src/shader_scene_bridge.rs
// (the frame publisher) and the Slint reference (shader_underlay.rs:44-45).
constexpr int SPECTRO_BANDS = 512;
constexpr int SPECTRO_COLS = 2048;
constexpr qsizetype FRAME_BYTES = 4 + 1 + SPECTRO_BANDS;

// The FULL 144-byte std140 scene block (spec 01 §1) — the ribbon reads only
// energyHi.y (the real-time ceiling line); the rest arrives zeroed except
// resolution.
struct UniformData
{
    float time;        //   0
    float phase;       //   4
    float beat;        //   8
    float level;       //  12
    float res[2];      //  16
    float levelSmooth; //  24
    float transient;   //  28
    float energyLo[4]; //  32
    float energyHi[4]; //  48
    float bandsLo[4];  //  64
    float bandsHi[4];  //  80
    float primary[4];  //  96
    float secondary[4];// 112
    float accent[4];   // 128
};
static_assert(sizeof(UniformData) == 144, "std140 scene block");

QShader loadShader(const QString &qrcPath)
{
    QFile f(qrcPath);
    if (f.open(QIODevice::ReadOnly)) {
        const QShader s = QShader::fromSerialized(f.readAll());
        if (!s.isValid())
            qWarning("[ribbon] shader %s failed to deserialize", qPrintable(qrcPath));
        return s;
    }
    qWarning("[ribbon] shader %s not found in the qrc", qPrintable(qrcPath));
    return QShader();
}

class RibbonRenderer : public QQuickRhiItemRenderer
{
public:
    void initialize(QRhiCommandBuffer *cb) override
    {
        QRhi *r = cb->rhi();
        if (m_vbuf) // already built (initialize can re-run on rhi changes)
            return;
        static const float TRI[] = { -1.0f, -1.0f, 3.0f, -1.0f, -1.0f, 3.0f };
        m_vbuf.reset(r->newBuffer(QRhiBuffer::Immutable,
                                  QRhiBuffer::VertexBuffer,
                                  quint32(sizeof(TRI))));
        m_ubuf.reset(r->newBuffer(QRhiBuffer::Dynamic,
                                  QRhiBuffer::UniformBuffer,
                                  quint32(sizeof(UniformData))));
        m_tex.reset(r->newTexture(QRhiTexture::R8, QSize(SPECTRO_BANDS, SPECTRO_COLS)));
        // Bilinear — the reference's `samp` smooths between bands/columns.
        m_sampler.reset(r->newSampler(QRhiSampler::Linear,
                                      QRhiSampler::Linear,
                                      QRhiSampler::None,
                                      QRhiSampler::ClampToEdge,
                                      QRhiSampler::ClampToEdge));
        if (!m_vbuf->create() || !m_ubuf->create() || !m_tex->create()
            || !m_sampler->create()) {
            qWarning("[ribbon] resource creation failed — the scene stays dark");
            m_vbuf.reset();
            return;
        }
        m_srb.reset(r->newShaderResourceBindings());
        m_srb->setBindings({
            QRhiShaderResourceBinding::uniformBuffer(
                0,
                QRhiShaderResourceBinding::VertexStage
                    | QRhiShaderResourceBinding::FragmentStage,
                m_ubuf.get()),
            // Binding 3, like the reference's @binding(3) spectrogram.
            QRhiShaderResourceBinding::sampledTexture(
                3, QRhiShaderResourceBinding::FragmentStage, m_tex.get(), m_sampler.get()),
        });
        if (!m_srb->create()) {
            qWarning("[ribbon] shader resource bindings failed — the scene stays dark");
            m_vbuf.reset();
            return;
        }
        QRhiResourceUpdateBatch *u = r->nextResourceUpdateBatch();
        u->uploadStaticBuffer(m_vbuf.get(), TRI);
        cb->resourceUpdate(u);
        m_needClear = true; // the texture starts UNDEFINED — clear on frame 1
    }

    void synchronize(QQuickRhiItem *item) override
    {
        // GUI thread: snapshot the item's properties (pulse tick only).
        auto *rb = static_cast<RibbonItem *>(item);
        m_frame = rb->m_frame;
        m_energyHi = rb->m_energyHi;
    }

    void render(QRhiCommandBuffer *cb) override
    {
        if (!m_vbuf)
            return;
        QRhiRenderTarget *rt = renderTarget();
        if (!m_pipeline || rt->renderPassDescriptor() != m_passDescriptor) {
            m_passDescriptor = rt->renderPassDescriptor();
            buildPipeline();
            if (!m_pipeline)
                return;
        }

        QRhiResourceUpdateBatch *u = rhi()->nextResourceUpdateBatch();

        if (m_needClear) {
            m_needClear = false;
            uploadClear(u);
        }

        // The pending frame: [col u32 LE][reset u8][512 row bytes].
        if (m_frame.size() == FRAME_BYTES) {
            const auto *p = reinterpret_cast<const uchar *>(m_frame.constData());
            const quint32 col = quint32(p[0]) | (quint32(p[1]) << 8)
                | (quint32(p[2]) << 16) | (quint32(p[3]) << 24);
            if (p[4] != 0)
                uploadClear(u);
            uploadRow(u, p + 5, int(col));
            m_frame.clear();
        }

        const QSize ps = rt->pixelSize();
        UniformData ud = {};
        ud.res[0] = float(ps.width());
        ud.res[1] = float(ps.height());
        std::memcpy(ud.energyHi, &m_energyHi, sizeof(ud.energyHi));
        u->updateDynamicBuffer(m_ubuf.get(), 0, quint32(sizeof(ud)), &ud);

        // Opaque clear (#03070c comes from the shader itself for the
        // margins; the clear colour only covers a degenerate frame).
        cb->beginPass(rt, QColor(3, 7, 12, 255), QRhiDepthStencilClearValue(1.0f, 0), u);
        cb->setGraphicsPipeline(m_pipeline.get());
        cb->setViewport(QRhiViewport(0, 0, float(ps.width()), float(ps.height())));
        cb->setShaderResources(m_srb.get());
        const QRhiCommandBuffer::VertexInput tri(m_vbuf.get(), 0);
        cb->setVertexInput(0, 1, &tri);
        cb->draw(3);
        cb->endPass();
    }

private:
    // Full-texture zero — track change / seek (shader_underlay.rs:859-883).
    // Resets the gap-fill cursor, like the reference's SPECTRO_LAST_COL.set(0).
    void uploadClear(QRhiResourceUpdateBatch *u)
    {
        m_lastCol = 0;
        const QByteArray zeros(SPECTRO_BANDS * SPECTRO_COLS, 0);
        QRhiTextureSubresourceUploadDescription sub(zeros);
        sub.setDataStride(SPECTRO_BANDS);
        u->uploadTexture(m_tex.get(),
                         QRhiTextureUploadDescription(QRhiTextureUploadEntry(0, 0, sub)));
    }

    // The row write + gap-fill (shader_underlay.rs:884-927): every column
    // skipped since the last write gets THIS row (progress updates ~1 Hz,
    // so the column jumps several slots between ticks).
    void uploadRow(QRhiResourceUpdateBatch *u, const uchar *row, int col)
    {
        col = qBound(0, col, SPECTRO_COLS - 1);
        const int start = col > m_lastCol ? m_lastCol + 1 : col;
        const int count = col + 1 - start;
        QByteArray data;
        data.resize(SPECTRO_BANDS * count);
        for (int i = 0; i < count; ++i)
            std::memcpy(data.data() + i * SPECTRO_BANDS, row, SPECTRO_BANDS);
        QRhiTextureSubresourceUploadDescription sub(data);
        sub.setDataStride(SPECTRO_BANDS);
        sub.setDestinationTopLeft(QPoint(0, start));
        // The partial upload NEEDS the explicit source size: without it the
        // deduced region is wrong (the rows never landed — the 2026-08-15
        // "axes but empty plot" bug; the full-texture clear painted fine).
        sub.setSourceSize(QSize(SPECTRO_BANDS, count));
        u->uploadTexture(m_tex.get(),
                         QRhiTextureUploadDescription(QRhiTextureUploadEntry(0, 0, sub)));
        m_lastCol = col;
    }

    void buildPipeline()
    {
        if (!m_pipeline)
            m_pipeline.reset(rhi()->newGraphicsPipeline());
        m_pipeline->setTopology(QRhiGraphicsPipeline::Triangles);
        m_pipeline->setCullMode(QRhiGraphicsPipeline::None);
        m_pipeline->setDepthTest(false);
        m_pipeline->setDepthWrite(false);
        // Opaque output (the FS writes alpha 1; the scene owns the
        // background while active).
        m_pipeline->setShaderStages({
            { QRhiShaderStage::Vertex,
              loadShader(QStringLiteral(
                  ":/qt/qml/com/blitzfc/qbz/qml/assets/shaders/rhi_triangle.vert.qsb")) },
            { QRhiShaderStage::Fragment,
              loadShader(QStringLiteral(
                  ":/qt/qml/com/blitzfc/qbz/qml/assets/shaders/spectral_ribbon.frag.qsb")) },
        });
        QRhiVertexInputLayout inputLayout;
        inputLayout.setBindings({ QRhiVertexInputBinding(quint32(2 * sizeof(float))) });
        inputLayout.setAttributes({
            { 0, 0, QRhiVertexInputAttribute::Float2, 0 }, // pos
        });
        m_pipeline->setVertexInputLayout(inputLayout);
        m_pipeline->setShaderResourceBindings(m_srb.get());
        m_pipeline->setRenderPassDescriptor(m_passDescriptor);
        if (!m_pipeline->create()) {
            qWarning("[ribbon] pipeline creation failed — the scene stays dark");
            m_pipeline.reset();
        }
    }

    std::unique_ptr<QRhiBuffer> m_vbuf;
    std::unique_ptr<QRhiBuffer> m_ubuf;
    std::unique_ptr<QRhiTexture> m_tex;
    std::unique_ptr<QRhiSampler> m_sampler;
    std::unique_ptr<QRhiShaderResourceBindings> m_srb;
    std::unique_ptr<QRhiGraphicsPipeline> m_pipeline;
    QRhiRenderPassDescriptor *m_passDescriptor = nullptr;

    QByteArray m_frame;
    QVector4D m_energyHi{ 0.0f, 0.0f, 0.0f, 0.0f };
    int m_lastCol = 0;
    bool m_needClear = false;
};

} // namespace

QQuickRhiItemRenderer *RibbonItem::createRenderer()
{
    return new RibbonRenderer;
}

// QML type registration — the linebed_item.cpp idiom: the module is STATIC,
// so a startup function runs at QGuiApplication construction, BEFORE the
// engine loads the QML that names RibbonItem.
static bool g_ribbonRegistered = false;

extern "C" void qbz_ribbon_register_qml_type()
{
    if (g_ribbonRegistered)
        return;
    g_ribbonRegistered = true;
    qmlRegisterType<RibbonItem>("com.blitzfc.qbz", 1, 0, "RibbonItem");
}

Q_COREAPP_STARTUP_FUNCTION(qbz_ribbon_register_qml_type)
