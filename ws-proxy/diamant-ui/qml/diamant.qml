pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import QtQuick.Window
import io.suntitle.diamant

ApplicationWindow {
    id: root

    readonly property color backgroundColor: "#faf8ff"
    readonly property color cardColor: "#ffffff"
    readonly property color borderColor: "#e5e5e5"
    readonly property color foregroundColor: "#0a0a0a"
    readonly property color mutedColor: "#737373"
    readonly property color violetColor: "#6e56cf"
    readonly property color violetTextColor: "#6550b9"
    readonly property color greenBackgroundColor: "#e6f6eb"
    readonly property color greenTextColor: "#218358"
    readonly property bool wideLayout: width >= 1000
    readonly property bool mediumLayout: width >= 720
    readonly property bool narrowLayout: width < 720
    readonly property int hubFieldColumns: wideLayout ? 3 : mediumLayout ? 2 : 1
    readonly property int runtimeFieldColumns: wideLayout ? 4 : mediumLayout ? 2 : 1

    width: 1200
    height: 800
    minimumWidth: 560
    minimumHeight: 650
    visible: true
    title: "Suntitle — Diamant"
    color: backgroundColor

    DiamantController {
        id: controller
    }
    ListModel {
        id: nodesModel
    }
    Connections {
        target: controller
        function onNodesJsonChanged() {
            nodesModel.clear();
            const nodes = JSON.parse(controller.nodesJson);
            for (let index = 0; index < nodes.length; index++)
                nodesModel.append(nodes[index]);
        }
    }

    component AppField: TextField {
        Layout.fillWidth: true
        implicitHeight: 32
        leftPadding: 10
        rightPadding: 10
        topPadding: 4
        bottomPadding: 4
        color: root.foregroundColor
        placeholderTextColor: "#a3a3a3"
        font.pixelSize: 13
        selectByMouse: true
        background: Rectangle {
            color: root.cardColor
            border.width: 1
            border.color: parent.activeFocus ? root.violetColor : root.borderColor
            radius: 10
        }
    }

    component FieldGroup: ColumnLayout {
        property alias label: fieldLabel.text
        spacing: 6
        Layout.fillWidth: true
        Label {
            id: fieldLabel
            color: root.mutedColor
            font.pixelSize: 12
            font.weight: Font.Medium
        }
    }

    component CardHeading: Item {
        property alias title: headingTitle.text
        property alias description: headingDescription.text
        property alias detail: headingDetail.text
        Layout.fillWidth: true
        Layout.preferredWidth: root.width - 96
        Layout.minimumWidth: root.width - 96
        Layout.maximumWidth: root.width - 96
        Layout.preferredHeight: 38
        Column {
            anchors.left: parent.left
            anchors.verticalCenter: parent.verticalCenter
            spacing: 2
            Label {
                id: headingTitle
                color: root.violetTextColor
                font.pixelSize: 15
                font.weight: Font.DemiBold
            }
            Label {
                id: headingDescription
                color: root.mutedColor
                font.pixelSize: 12
            }
        }
        Label {
            id: headingDetail
            anchors.right: parent.right
            anchors.verticalCenter: parent.verticalCenter
            visible: !root.narrowLayout
            color: root.violetTextColor
            font.pixelSize: 12
            font.weight: Font.Medium
        }
    }

    header: Rectangle {
        height: 64
        color: "#202020"
        RowLayout {
            anchors.fill: parent
            anchors.leftMargin: 24
            anchors.rightMargin: 24
            spacing: 12
            RowLayout {
                Layout.preferredWidth: root.narrowLayout ? 190 : 300
                Layout.preferredHeight: 40
                spacing: 12
                Rectangle {
                    Layout.preferredWidth: 36
                    Layout.preferredHeight: 36
                    radius: 8
                    color: root.violetColor
                    Image {
                        anchors.centerIn: parent
                        width: 18
                        height: 18
                        source: "qrc:/qt/qml/io/suntitle/diamant/qml/assets/diamant.svg"
                    }
                }
                ColumnLayout {
                    spacing: 0
                    Label {
                        text: "Diamant"
                        color: "#fcfcfc"
                        font.pixelSize: 16
                        font.weight: Font.DemiBold
                    }
                    Label {
                        text: "Suntitle host agent"
                        visible: !root.narrowLayout
                        color: "#adadad"
                        font.pixelSize: 12
                    }
                }
            }
            Item {
                Layout.fillWidth: true
            }
            Rectangle {
                visible: !root.narrowLayout
                implicitWidth: connectionBadge.implicitWidth + 16
                implicitHeight: 22
                radius: 11
                color: controller.running ? root.greenBackgroundColor : root.backgroundColor
                Label {
                    id: connectionBadge
                    anchors.centerIn: parent
                    text: controller.connectionState === "connected" ? "Connected" : controller.connectionState === "connecting" ? "Connecting…" : controller.connectionState === "disconnecting" ? "Disconnecting…" : controller.connectionState === "error" ? "Connection error" : "Not connected"
                    color: controller.running ? root.greenTextColor : root.violetTextColor
                    font.pixelSize: 12
                    font.weight: Font.Medium
                }
                ToolTip.visible: badgeMouse.containsMouse && controller.connectionMessage.length > 0
                ToolTip.text: controller.connectionMessage
                MouseArea {
                    id: badgeMouse
                    anchors.fill: parent
                    hoverEnabled: true
                }
            }
            Button {
                id: connectButton
                implicitHeight: 36
                implicitWidth: connectLabel.implicitWidth + 20
                text: controller.running ? "Disconnect" : "Connect to hub"
                onClicked: controller.toggleConnection()
                contentItem: Label {
                    id: connectLabel
                    text: connectButton.text
                    color: controller.running ? root.violetTextColor : "#fcfcfc"
                    font.pixelSize: 14
                    font.weight: Font.Medium
                    horizontalAlignment: Text.AlignHCenter
                    verticalAlignment: Text.AlignVCenter
                }
                background: Rectangle {
                    color: controller.running ? "#fcfcfc" : connectButton.down ? "#5f49bd" : connectButton.hovered ? "#765fda" : root.violetColor
                    border.width: controller.running ? 1 : 0
                    border.color: root.violetColor
                    radius: 10
                }
            }
        }
    }

    ScrollView {
        anchors.fill: parent
        contentWidth: availableWidth
        clip: true
        ColumnLayout {
            width: root.width
            spacing: 16
            Item {
                Layout.preferredHeight: 8
            }
            Rectangle {
                color: root.cardColor
                border.color: root.borderColor
                border.width: 1
                radius: 8
                Layout.fillWidth: true
                Layout.leftMargin: 24
                Layout.rightMargin: 24
                Layout.preferredHeight: 154 + (Math.ceil(3 / root.hubFieldColumns) - 1) * 66
                ColumnLayout {
                    anchors.fill: parent
                    anchors.leftMargin: 24
                    anchors.rightMargin: 24
                    anchors.topMargin: 18
                    anchors.bottomMargin: 18
                    spacing: 12
                    CardHeading {
                        title: "Hub connection"
                        description: controller.running ? "Connected to the Suntitle hub. Changes are saved locally." : "Configure the hub and host identity, then connect. Settings are saved locally."
                        detail: controller.running ? "Heartbeat: just now" : "Ready to connect"
                    }
                    GridLayout {
                        Layout.fillWidth: true
                        Layout.preferredWidth: parent.width
                        columns: root.hubFieldColumns
                        columnSpacing: 12
                        rowSpacing: 12
                        FieldGroup {
                            label: "Proxy URL"
                            Layout.preferredWidth: 1
                            AppField {
                                text: controller.proxyUrl
                                onEditingFinished: controller.proxyUrl = text
                            }
                        }
                        FieldGroup {
                            label: "Host name"
                            Layout.preferredWidth: 1
                            AppField {
                                text: controller.hostName
                                readOnly: true
                            }
                        }
                        FieldGroup {
                            label: "Host IP"
                            Layout.preferredWidth: 1
                            AppField {
                                text: controller.hostIp
                                placeholderText: "Automatic"
                                onEditingFinished: controller.hostIp = text
                            }
                        }
                    }
                }
            }
            Rectangle {
                color: root.cardColor
                border.color: root.borderColor
                border.width: 1
                radius: 8
                Layout.fillWidth: true
                Layout.leftMargin: 24
                Layout.rightMargin: 24
                Layout.preferredHeight: 154 + (Math.ceil(4 / root.runtimeFieldColumns) - 1) * 66
                ColumnLayout {
                    anchors.fill: parent
                    anchors.leftMargin: 24
                    anchors.rightMargin: 24
                    anchors.topMargin: 18
                    anchors.bottomMargin: 18
                    spacing: 12
                    CardHeading {
                        title: "Runtime settings"
                        description: "These values replace the remaining Diamant CLI arguments."
                        detail: "Saved on this device"
                    }
                    GridLayout {
                        Layout.fillWidth: true
                        Layout.preferredWidth: parent.width
                        columns: root.runtimeFieldColumns
                        columnSpacing: 8
                        rowSpacing: 12
                        FieldGroup {
                            label: "Project directory"
                            Layout.preferredWidth: root.wideLayout ? 390 : 1
                            AppField {
                                text: controller.projectDir
                                onEditingFinished: controller.projectDir = text
                            }
                        }
                        FieldGroup {
                            label: "Data-plane URL"
                            Layout.preferredWidth: root.wideLayout ? 390 : 1
                            AppField {
                                text: controller.dataPlaneUrl
                                placeholderText: "Derived from proxy URL"
                                onEditingFinished: controller.dataPlaneUrl = text
                            }
                        }
                        FieldGroup {
                            label: "Health interval"
                            Layout.preferredWidth: root.wideLayout ? 150 : 1
                            Layout.fillWidth: true
                            AppField {
                                text: activeFocus ? controller.healthInterval.toString() : controller.healthInterval + " s"
                                inputMethodHints: Qt.ImhDigitsOnly
                                onEditingFinished: {
                                    const parsed = parseInt(text);
                                    if (!isNaN(parsed) && parsed > 0)
                                        controller.healthInterval = parsed;
                                }
                            }
                        }
                        FieldGroup {
                            label: "Shutdown grace"
                            Layout.preferredWidth: root.wideLayout ? 150 : 1
                            Layout.fillWidth: true
                            AppField {
                                text: activeFocus ? controller.shutdownGrace.toString() : controller.shutdownGrace + " s"
                                inputMethodHints: Qt.ImhDigitsOnly
                                onEditingFinished: {
                                    const parsed = parseInt(text);
                                    if (!isNaN(parsed) && parsed > 0)
                                        controller.shutdownGrace = parsed;
                                }
                            }
                        }
                    }
                }
            }
            Rectangle {
                color: root.cardColor
                border.color: root.borderColor
                border.width: 1
                radius: 8
                Layout.fillWidth: true
                Layout.leftMargin: 24
                Layout.rightMargin: 24
                Layout.bottomMargin: 24
                Layout.preferredHeight: 348
                ColumnLayout {
                    anchors.fill: parent
                    spacing: 0
                    Item {
                        Layout.fillWidth: true
                        Layout.preferredWidth: root.width - 96
                        Layout.minimumWidth: root.width - 96
                        Layout.maximumWidth: root.width - 96
                        Layout.preferredHeight: 64
                        Layout.leftMargin: 24
                        Layout.rightMargin: 24
                        Column {
                            anchors.left: parent.left
                            anchors.verticalCenter: parent.verticalCenter
                            spacing: 2
                            Label {
                                text: "Running nodes"
                                color: root.violetTextColor
                                font.pixelSize: 15
                                font.weight: Font.DemiBold
                            }
                            Label {
                                text: controller.running ? "Processes currently managed by this Diamant instance." : "Connect to the hub to load and manage assigned nodes."
                                color: root.mutedColor
                                font.pixelSize: 12
                            }
                        }
                        Rectangle {
                            anchors.right: parent.right
                            anchors.verticalCenter: parent.verticalCenter
                            implicitWidth: nodeBadge.implicitWidth + 16
                            implicitHeight: 22
                            radius: 11
                            color: nodesModel.count > 0 ? root.greenBackgroundColor : root.backgroundColor
                            Label {
                                id: nodeBadge
                                anchors.centerIn: parent
                                text: nodesModel.count + " running"
                                color: nodesModel.count > 0 ? root.greenTextColor : root.violetTextColor
                                font.pixelSize: 12
                                font.weight: Font.Medium
                            }
                        }
                    }
                    Rectangle {
                        Layout.fillWidth: true
                        Layout.preferredHeight: 1
                        color: root.borderColor
                    }
                    ColumnLayout {
                        visible: nodesModel.count > 0
                        Layout.fillWidth: true
                        Layout.preferredWidth: parent.width - 48
                        Layout.fillHeight: true
                        Layout.leftMargin: 24
                        Layout.rightMargin: 24
                        spacing: 0
                        Rectangle {
                            Layout.fillWidth: true
                            Layout.preferredHeight: 44
                            color: root.backgroundColor
                            RowLayout {
                                anchors.fill: parent
                                spacing: 0
                                Repeater {
                                    model: ["Name", "Type", "Status", "Command parameters"]
                                    Label {
                                        required property int index
                                        required property string modelData
                                        text: modelData
                                        color: root.foregroundColor
                                        font.pixelSize: 14
                                        font.weight: Font.Medium
                                        leftPadding: 8
                                        Layout.preferredWidth: index === 0 ? 210 : index === 1 ? 160 : index === 2 ? 120 : 200
                                        Layout.fillWidth: index === 3
                                        elide: Text.ElideRight
                                    }
                                }
                            }
                        }
                        Repeater {
                            model: nodesModel
                            Rectangle {
                                id: nodeRow
                                required property string name
                                required property string node_type
                                required property string status
                                required property var parameters
                                Layout.fillWidth: true
                                Layout.preferredHeight: 56
                                color: root.cardColor
                                RowLayout {
                                    anchors.fill: parent
                                    spacing: 0
                                    Label {
                                        text: nodeRow.name
                                        color: root.foregroundColor
                                        font.pixelSize: 14
                                        font.weight: Font.Medium
                                        leftPadding: 8
                                        Layout.preferredWidth: 210
                                        elide: Text.ElideRight
                                    }
                                    Label {
                                        text: nodeRow.node_type
                                        color: root.foregroundColor
                                        font.pixelSize: 14
                                        leftPadding: 8
                                        Layout.preferredWidth: 160
                                        elide: Text.ElideRight
                                    }
                                    Label {
                                        text: nodeRow.status
                                        color: nodeRow.status === "Running" ? root.greenTextColor : nodeRow.status === "Failed" ? "#dc2626" : root.mutedColor
                                        font.pixelSize: 14
                                        leftPadding: 8
                                        Layout.preferredWidth: 120
                                    }
                                    Label {
                                        text: nodeRow.parameters.join(" ")
                                        color: root.foregroundColor
                                        font.pixelSize: 14
                                        leftPadding: 8
                                        Layout.fillWidth: true
                                        elide: Text.ElideRight
                                    }
                                }
                            }
                        }
                        Item {
                            Layout.fillHeight: true
                        }
                    }
                    ColumnLayout {
                        visible: nodesModel.count === 0
                        Layout.fillWidth: true
                        Layout.preferredWidth: parent.width
                        Layout.fillHeight: true
                        spacing: 10
                        Item {
                            Layout.fillHeight: true
                        }
                        Rectangle {
                            Layout.alignment: Qt.AlignHCenter
                            Layout.preferredWidth: 44
                            Layout.preferredHeight: 44
                            radius: 8
                            color: root.backgroundColor
                            Image {
                                anchors.centerIn: parent
                                width: 20
                                height: 20
                                source: "qrc:/qt/qml/io/suntitle/diamant/qml/assets/empty-nodes.svg"
                            }
                        }
                        Label {
                            Layout.alignment: Qt.AlignHCenter
                            text: "No running nodes"
                            color: root.foregroundColor
                            font.pixelSize: 14
                            font.weight: Font.DemiBold
                        }
                        Label {
                            Layout.alignment: Qt.AlignHCenter
                            text: controller.running ? "No nodes are currently assigned to this host." : "Connect to the hub to receive node assignments."
                            color: root.mutedColor
                            font.pixelSize: 12
                        }
                        Item {
                            Layout.fillHeight: true
                        }
                    }
                }
            }
        }
    }
}
