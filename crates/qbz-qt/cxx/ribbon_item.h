// Spectral Ribbon (immersive shader scene, mode 4) — the QQuickRhiItem
// behind the GPU spectrogram. Block A3 of the 2026-08-15
// immersive-completion contract (spec 01 §2.4). Third hand-written C++
// item after LineBedItem/PlasmaItem (cxx/linebed_item.h carries the
// why-C++ rationale; this scene additionally needs a PERSISTENT texture
// written in sub-rects, which a ShaderEffect cannot express).
//
// QML properties are MEMBER-only (no NOTIFY): the QML wrapper
// (RibbonScene.qml) writes them on the pulse edge and calls update()
// itself — the same cadence contract as LineBedItem/PlasmaItem.
//
// `frame` is ONE QByteArray per viz tick, self-describing:
//   bytes 0..4   column (u32 LE) — playback-time column, 0..2047
//   byte  4      reset flag (0/1) — full spectrogram clear (track
//                change / seek), applied BEFORE the row write
//   bytes 5..517 the 512-band row (u8 per band, dB-scaled host-side)
// (the pack's batching pattern: ONE property = ONE notify per tick).
//
// Registered as QML type `RibbonItem` in `com.blitzfc.qbz` 1.0 (see the
// .cpp).

#pragma once

#include <QtCore/QByteArray>
#include <QtGui/QVector4D>
#include <QtQuick/QQuickRhiItem>

class RibbonItem : public QQuickRhiItem
{
    Q_OBJECT
    Q_PROPERTY(QByteArray frame MEMBER m_frame)
    Q_PROPERTY(QVector4D energyHi MEMBER m_energyHi)

public:
    explicit RibbonItem(QQuickItem *parent = nullptr) : QQuickRhiItem(parent) {}

    QQuickRhiItemRenderer *createRenderer() override;

    QByteArray m_frame;
    QVector4D m_energyHi{ 0.0f, 0.0f, 0.0f, 0.0f };
};
