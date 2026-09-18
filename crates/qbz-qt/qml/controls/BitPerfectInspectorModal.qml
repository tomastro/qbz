import QtQuick
import QtQuick.Controls
import com.blitzfc.qbz
import "../theme"

Item {
    id: root

    property bool isOpen: false
    property var usbStatus: ({})
    property string lastCheckTime: ""

    anchors.fill: parent
    visible: opacity > 0
    opacity: isOpen ? 1 : 0
    z: 9999

    Behavior on opacity {
        NumberAnimation { duration: 200; easing.type: Easing.OutQuad }
    }

    QbzTheme { id: theme }

    function open() {
        refreshStatus();
        isOpen = true;
    }

    function close() {
        isOpen = false;
    }

    function refreshStatus() {
        try {
            var raw = QbzBridge.getAndroidUsbStatus();
            if (raw && raw.length > 0) {
                root.usbStatus = JSON.parse(raw);
            } else {
                root.usbStatus = {};
            }
        } catch (e) {
            root.usbStatus = {};
        }
        var now = new Date();
        var h = ("0" + now.getHours()).slice(-2);
        var m = ("0" + now.getMinutes()).slice(-2);
        var s = ("0" + now.getSeconds()).slice(-2);
        root.lastCheckTime = h + ":" + m + ":" + s;
    }

    // Helper properties
    readonly property bool isAndroid: root.usbStatus.is_android === true || Qt.platform.os === "android"
    readonly property bool usbConnected: root.usbStatus.usb_connected === true
    readonly property bool hasPermission: root.usbStatus.has_permission === true
    readonly property bool isStreamOpen: root.usbStatus.is_stream_open === true
    readonly property bool isBpActive: {
        if (!isAndroid) return QbzPlayer.npBitPerfectMode === "direct";
        return (root.usbConnected && root.isStreamOpen) || (QbzPlayer.npBitPerfectMode === "direct");
    }

    // Backdrop
    Rectangle {
        anchors.fill: parent
        color: Qt.rgba(0, 0, 0, 0.65)

        MouseArea {
            anchors.fill: parent
            onClicked: root.close()
        }
    }

    // Modal Card
    Rectangle {
        id: card
        width: Math.min(parent.width - 24, 460)
        height: Math.min(parent.height * 0.88, contentCol.implicitHeight + 40)
        anchors.centerIn: parent
        radius: 20
        color: "#18181b"
        border.width: 1
        border.color: Qt.rgba(1, 1, 1, 0.12)
        clip: true

        MouseArea {
            anchors.fill: parent
            // prevent closing when clicking inside
        }

        Flickable {
            id: flick
            anchors.fill: parent
            anchors.margins: 18
            contentWidth: width
            contentHeight: contentCol.implicitHeight
            clip: true
            boundsBehavior: Flickable.StopAtBounds

            Column {
                id: contentCol
                width: parent.width
                spacing: 16

                // 1. Header
                Item {
                    width: parent.width
                    height: 36

                    Row {
                        anchors.left: parent.left
                        anchors.verticalCenter: parent.verticalCenter
                        spacing: 10

                        Rectangle {
                            width: 28
                            height: 28
                            radius: 14
                            color: root.isBpActive ? Qt.rgba(0.18, 0.8, 0.44, 0.2) : Qt.rgba(1, 1, 1, 0.1)

                            Text {
                                anchors.centerIn: parent
                                text: "BP"
                                font.pixelSize: 11
                                font.weight: Font.Bold
                                color: root.isBpActive ? "#2ecc71" : "#a1a1aa"
                            }
                        }

                        Text {
                            anchors.verticalCenter: parent.verticalCenter
                            text: "Bit-Perfect 動作診断"
                            font.pixelSize: 18
                            font.weight: Font.Bold
                            color: "#ffffff"
                        }
                    }

                    // Close Button
                    Rectangle {
                        anchors.right: parent.right
                        anchors.verticalCenter: parent.verticalCenter
                        width: 32
                        height: 32
                        radius: 16
                        color: closeArea.containsMouse ? Qt.rgba(1, 1, 1, 0.18) : Qt.rgba(1, 1, 1, 0.08)

                        Text {
                            anchors.centerIn: parent
                            text: "✕"
                            font.pixelSize: 14
                            color: "#ffffff"
                        }

                        MouseArea {
                            id: closeArea
                            anchors.fill: parent
                            cursorShape: Qt.PointingHandCursor
                            onClicked: root.close()
                        }
                    }
                }

                // 2. Main Status Banner
                Rectangle {
                    width: parent.width
                    radius: 14
                    border.width: 1
                    implicitHeight: bannerCol.implicitHeight + 24

                    color: {
                        if (root.isBpActive) return Qt.rgba(0.18, 0.8, 0.44, 0.15);
                        if (root.usbConnected && root.hasPermission) return Qt.rgba(0.2, 0.6, 0.86, 0.15);
                        if (root.usbConnected && !root.hasPermission) return Qt.rgba(0.9, 0.49, 0.13, 0.15);
                        return Qt.rgba(1, 1, 1, 0.06);
                    }
                    border.color: {
                        if (root.isBpActive) return "#2ecc71";
                        if (root.usbConnected && root.hasPermission) return "#3498db";
                        if (root.usbConnected && !root.hasPermission) return "#e67e22";
                        return Qt.rgba(1, 1, 1, 0.15);
                    }

                    Column {
                        id: bannerCol
                        anchors.fill: parent
                        anchors.margins: 14
                        spacing: 6

                        Row {
                            spacing: 8
                            Rectangle {
                                width: 10
                                height: 10
                                radius: 5
                                anchors.verticalCenter: parent.verticalCenter
                                color: root.isBpActive ? "#2ecc71" : (root.usbConnected ? "#3498db" : "#71717a")
                            }
                            Text {
                                text: {
                                    if (root.isBpActive) return "ビットパーフェクト (BP) 成立中";
                                    if (root.usbConnected && root.hasPermission) return "USB DAC 接続完了（再生待機中）";
                                    if (root.usbConnected && !root.hasPermission) return "USB アクセス権限の承認が必要です";
                                    return "USB DAC 未接続（システム出力）";
                                }
                                font.pixelSize: 15
                                font.weight: Font.Bold
                                color: root.isBpActive ? "#2ecc71" : (root.usbConnected ? "#60a5fa" : "#ffffff")
                            }
                        }

                        Text {
                            width: parent.width
                            wrapMode: Text.WordWrap
                            font.pixelSize: 12
                            lineHeight: 1.3
                            color: Qt.rgba(1, 1, 1, 0.75)
                            text: {
                                if (root.isBpActive) {
                                    return "Android OSのミキサー（AudioFlinger）を完全バイパスし、楽曲のオリジナルPCMデータを1ビットも損なわずにDACへダイレクト転送しています。";
                                }
                                if (root.usbConnected && root.hasPermission) {
                                    return "USB DACが正常に認識されています。楽曲を再生すると音源フォーマットに合わせたビットパーフェクト再生が開始されます。";
                                }
                                if (root.usbConnected && !root.hasPermission) {
                                    return "DACの排他直接制御（usbfs）を行うためのAndroid USB権限が未承認です。下のボタンから権限を許可してください。";
                                }
                                return "Android端末のUSB-C端子にUSB DAC/アンプを接続すると、自動的にハードウェア直接転送（ビットパーフェクト）が有効になります。";
                            }
                        }

                        // Permission request button if needed
                        Rectangle {
                            visible: root.usbConnected && !root.hasPermission
                            width: parent.width
                            height: 36
                            radius: 8
                            color: "#e67e22"

                            Text {
                                anchors.centerIn: parent
                                text: "USB アクセス権限をリクエスト"
                                font.pixelSize: 13
                                font.weight: Font.Bold
                                color: "#ffffff"
                            }

                            MouseArea {
                                anchors.fill: parent
                                onClicked: {
                                    QbzBridge.requestUsbPermission();
                                    root.refreshStatus();
                                }
                            }
                        }
                    }
                }

                // 3. Signal Chain Verification Table
                Column {
                    width: parent.width
                    spacing: 8

                    Text {
                        text: "シグナルチェーン照合（音源 vs 出力）"
                        font.pixelSize: 13
                        font.weight: Font.Bold
                        color: Qt.rgba(1, 1, 1, 0.6)
                    }

                    Rectangle {
                        width: parent.width
                        radius: 12
                        color: Qt.rgba(1, 1, 1, 0.04)
                        border.width: 1
                        border.color: Qt.rgba(1, 1, 1, 0.08)
                        implicitHeight: tableCol.implicitHeight + 20

                        Column {
                            id: tableCol
                            anchors.fill: parent
                            anchors.margins: 12
                            spacing: 10

                            // Row A: Sampling Rate
                            Row {
                                width: parent.width
                                Text {
                                    width: 110
                                    text: "サンプリング周波数"
                                    font.pixelSize: 12
                                    color: Qt.rgba(1, 1, 1, 0.55)
                                }
                                Text {
                                    width: parent.width - 110
                                    wrapMode: Text.WordWrap
                                    font.pixelSize: 12
                                    color: "#ffffff"
                                    text: {
                                        var src = QbzPlayer.npEffRateHz > 0 ? (QbzPlayer.npEffRateHz / 1000) + " kHz" : "---";
                                        var dac = root.usbStatus.active_rate > 0 ? (root.usbStatus.active_rate / 1000) + " kHz" : (root.usbConnected ? "ネイティブ追従" : "OS共有");
                                        var match = (root.usbStatus.active_rate > 0 && root.usbStatus.active_rate === QbzPlayer.npEffRateHz) ? " [完全一致]" : "";
                                        return src + "  ➔  " + dac + match;
                                    }
                                }
                            }

                            Rectangle { width: parent.width; height: 1; color: Qt.rgba(1, 1, 1, 0.06) }

                            // Row B: Bit Depth
                            Row {
                                width: parent.width
                                Text {
                                    width: 110
                                    text: "量子化ビット数"
                                    font.pixelSize: 12
                                    color: Qt.rgba(1, 1, 1, 0.55)
                                }
                                Text {
                                    width: parent.width - 110
                                    wrapMode: Text.WordWrap
                                    font.pixelSize: 12
                                    color: "#ffffff"
                                    text: {
                                        var src = QbzPlayer.npEffBits > 0 ? QbzPlayer.npEffBits + "-bit" : "---";
                                        var dac = root.usbStatus.active_bits > 0 ? root.usbStatus.active_bits + "-bit" : (root.usbConnected ? "ネイティブ" : "OS共有");
                                        var match = (root.usbStatus.active_bits > 0 && root.usbStatus.active_bits === QbzPlayer.npEffBits) ? " [完全一致]" : "";
                                        return src + "  ➔  " + dac + match;
                                    }
                                }
                            }

                            Rectangle { width: parent.width; height: 1; color: Qt.rgba(1, 1, 1, 0.06) }

                            // Row C: OS Mixer
                            Row {
                                width: parent.width
                                Text {
                                    width: 110
                                    text: "Android OSミキサー"
                                    font.pixelSize: 12
                                    color: Qt.rgba(1, 1, 1, 0.55)
                                }
                                Text {
                                    width: parent.width - 110
                                    font.pixelSize: 12
                                    color: root.isBpActive ? "#2ecc71" : "#a1a1aa"
                                    text: root.isBpActive ? "完全バイパス (AudioFlinger 非介入)" : (root.usbConnected ? "バイパス待機" : "OSミキサー経由")
                                }
                            }

                            Rectangle { width: parent.width; height: 1; color: Qt.rgba(1, 1, 1, 0.06) }

                            // Row D: Software Volume
                            Row {
                                width: parent.width
                                Text {
                                    width: 110
                                    text: "ソフトウェア音量"
                                    font.pixelSize: 12
                                    color: Qt.rgba(1, 1, 1, 0.55)
                                }
                                Text {
                                    width: parent.width - 110
                                    font.pixelSize: 12
                                    color: root.isBpActive ? "#2ecc71" : "#a1a1aa"
                                    text: root.isBpActive ? "100% ロック (ビット完全保持)" : "通常制御"
                                }
                            }

                            Rectangle { width: parent.width; height: 1; color: Qt.rgba(1, 1, 1, 0.06) }

                            // Row E: Transfer Protocol
                            Row {
                                width: parent.width
                                Text {
                                    width: 110
                                    text: "伝送方式"
                                    font.pixelSize: 12
                                    color: Qt.rgba(1, 1, 1, 0.55)
                                }
                                Text {
                                    width: parent.width - 110
                                    font.pixelSize: 12
                                    color: "#ffffff"
                                    text: root.usbConnected ? "USB Audio Class 2.0 (usbfs direct)" : "Android AudioTrack"
                                }
                            }
                        }
                    }
                }

                // 4. Hardware Details
                Column {
                    width: parent.width
                    spacing: 8

                    Text {
                        text: "接続 DAC ハードウェア仕様"
                        font.pixelSize: 13
                        font.weight: Font.Bold
                        color: Qt.rgba(1, 1, 1, 0.6)
                    }

                    Rectangle {
                        width: parent.width
                        radius: 12
                        color: Qt.rgba(1, 1, 1, 0.04)
                        border.width: 1
                        border.color: Qt.rgba(1, 1, 1, 0.08)
                        implicitHeight: hwCol.implicitHeight + 20

                        Column {
                            id: hwCol
                            anchors.fill: parent
                            anchors.margins: 12
                            spacing: 8

                            Row {
                                width: parent.width
                                Text { width: 100; text: "デバイス名"; font.pixelSize: 12; color: Qt.rgba(1, 1, 1, 0.55) }
                                Text {
                                    width: parent.width - 100
                                    font.pixelSize: 12
                                    font.weight: Font.Medium
                                    color: "#ffffff"
                                    elide: Text.ElideRight
                                    text: root.usbStatus.device_name || (root.usbConnected ? "USB Audio Device" : "未検出")
                                }
                            }

                            Row {
                                visible: root.usbConnected && root.usbStatus.vendor_id > 0
                                width: parent.width
                                Text { width: 100; text: "VID / PID"; font.pixelSize: 12; color: Qt.rgba(1, 1, 1, 0.55) }
                                Text {
                                    width: parent.width - 100
                                    font.pixelSize: 12
                                    color: "#a1a1aa"
                                    text: "0x" + Number(root.usbStatus.vendor_id || 0).toString(16) + " / 0x" + Number(root.usbStatus.product_id || 0).toString(16)
                                }
                            }

                            Row {
                                visible: root.usbConnected
                                width: parent.width
                                Text { width: 100; text: "クロック制御"; font.pixelSize: 12; color: Qt.rgba(1, 1, 1, 0.55) }
                                Text {
                                    width: parent.width - 100
                                    font.pixelSize: 12
                                    color: "#ffffff"
                                    text: "UAC2 非同期フィードバック (Async)"
                                }
                            }

                            Row {
                                visible: root.usbConnected && root.usbStatus.supported_rates && root.usbStatus.supported_rates.length > 0
                                width: parent.width
                                Text { width: 100; text: "対応レート"; font.pixelSize: 12; color: Qt.rgba(1, 1, 1, 0.55) }
                                Text {
                                    width: parent.width - 100
                                    wrapMode: Text.WordWrap
                                    font.pixelSize: 12
                                    color: "#a1a1aa"
                                    text: (root.usbStatus.supported_rates || []).map(function(r) { return (r/1000) + "k" }).join(" / ")
                                }
                            }
                        }
                    }
                }

                // 5. Action Buttons (Refresh / Re-diagnose)
                Row {
                    width: parent.width
                    spacing: 10

                    Rectangle {
                        width: parent.width - 110
                        height: 40
                        radius: 10
                        color: refreshArea.containsMouse ? Qt.rgba(1, 1, 1, 0.18) : Qt.rgba(1, 1, 1, 0.1)

                        Row {
                            anchors.centerIn: parent
                            spacing: 6
                            Text {
                                text: "↻"
                                font.pixelSize: 14
                                color: "#ffffff"
                            }
                            Text {
                                text: "最新状態を再診断 (" + (root.lastCheckTime || "更新") + ")"
                                font.pixelSize: 13
                                font.weight: Font.Medium
                                color: "#ffffff"
                            }
                        }

                        MouseArea {
                            id: refreshArea
                            anchors.fill: parent
                            cursorShape: Qt.PointingHandCursor
                            onClicked: root.refreshStatus()
                        }
                    }

                    Rectangle {
                        width: 100
                        height: 40
                        radius: 10
                        color: Qt.rgba(1, 1, 1, 0.1)

                        Text {
                            anchors.centerIn: parent
                            text: "閉じる"
                            font.pixelSize: 13
                            color: "#ffffff"
                        }

                        MouseArea {
                            anchors.fill: parent
                            cursorShape: Qt.PointingHandCursor
                            onClicked: root.close()
                        }
                    }
                }
            }
        }
    }
}
