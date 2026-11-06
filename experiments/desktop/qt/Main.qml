import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

ApplicationWindow {
    id: root
    required property QtObject client
    width: 760
    height: 520
    visible: true
    title: "AuroDownloader"

    // Fermer la fenêtre la replie dans la zone de notification (F-36).
    onClosing: (close) => { close.accepted = false; root.hide() }

    function bytes(n) {
        if (n < 0) return "?"
        const units = [qsTr("o"), qsTr("Kio"), qsTr("Mio"), qsTr("Gio"), qsTr("Tio")]
        let i = 0
        while (n >= 1024 && i < units.length - 1) { n /= 1024; i++ }
        return Number(n).toLocaleString(Qt.locale(), "f", i === 0 ? 0 : 1) + " " + units[i]
    }
    function duration(s) {
        if (s < 0) return ""
        if (s < 60) return qsTr("%1 s").arg(s)
        if (s < 3600) return qsTr("%1 min %2 s").arg(Math.floor(s / 60)).arg(s % 60)
        return qsTr("%1 h %2 min").arg(Math.floor(s / 3600)).arg(Math.floor(s % 3600 / 60))
    }
    function stateName(state) {
        switch (state) {
        case "queued": return qsTr("En attente")
        case "active": return qsTr("En cours")
        case "paused": return qsTr("En pause")
        case "retry_wait": return qsTr("Nouvel essai bientôt")
        case "completed": return qsTr("Terminé")
        case "failed": return qsTr("Échec")
        case "cancelled": return qsTr("Annulé")
        default: return state
        }
    }

    Shortcut { sequence: "Ctrl+L"; onActivated: url.forceActiveFocus() }

    header: ToolBar {
        RowLayout {
            anchors.fill: parent
            anchors.margins: 6
            TextField {
                id: url
                Layout.fillWidth: true
                placeholderText: qsTr("Adresse à télécharger")
                Accessible.name: qsTr("Adresse à télécharger")
                focus: true
                onAccepted: add.clicked()
            }
            Button {
                id: add
                text: qsTr("Ajouter")
                Accessible.name: qsTr("Ajouter le téléchargement")
                enabled: url.text.length > 0 && root.client.connected
                onClicked: { root.client.add(url.text); url.clear() }
            }
        }
    }

    ColumnLayout {
        anchors.fill: parent
        anchors.margins: 6
        spacing: 4

        Label {
            Layout.fillWidth: true
            visible: text.length > 0
            text: root.client.lastError
            color: "#c0392b"
            wrapMode: Text.Wrap
            Accessible.role: Accessible.AlertMessage
        }

        ListView {
            id: list
            Layout.fillWidth: true
            Layout.fillHeight: true
            clip: true
            model: root.client.tasks
            spacing: 2
            keyNavigationEnabled: true
            activeFocusOnTab: true
            Accessible.name: qsTr("Téléchargements")
            Accessible.role: Accessible.List
            ScrollBar.vertical: ScrollBar { id: scroll }

            delegate: Frame {
                id: row
                required property int index
                required property string taskId
                required property string name
                required property string state
                required property double done
                required property double total
                required property double bps
                required property double eta
                width: ListView.view.width - scroll.width
                padding: 6
                activeFocusOnTab: false
                Accessible.role: Accessible.ListItem
                Accessible.name: qsTr("%1, %2, %3 sur %4")
                    .arg(name).arg(root.stateName(state)).arg(root.bytes(done)).arg(root.bytes(total))
                background: Rectangle {
                    color: row.ListView.isCurrentItem && list.activeFocus ? palette.highlight : palette.base
                    opacity: row.ListView.isCurrentItem && list.activeFocus ? 0.25 : 1
                    border.color: palette.mid
                    radius: 3
                }

                RowLayout {
                    anchors.fill: parent
                    spacing: 8
                    ColumnLayout {
                        Layout.fillWidth: true
                        Label { text: row.name; elide: Text.ElideMiddle; Layout.fillWidth: true; font.bold: true }
                        ProgressBar {
                            Layout.fillWidth: true
                            from: 0
                            to: row.total > 0 ? row.total : 1
                            value: row.total > 0 ? row.done : 0
                            indeterminate: row.total < 0 && row.state === "active"
                            Accessible.name: qsTr("Avancement de %1").arg(row.name)
                        }
                        Label {
                            Layout.fillWidth: true
                            opacity: 0.8
                            text: [root.stateName(row.state),
                                   root.bytes(row.done) + " / " + root.bytes(row.total),
                                   row.state === "active" ? root.bytes(row.bps) + qsTr("/s") : "",
                                   root.duration(row.eta)].filter(s => s.length > 0).join(" · ")
                        }
                    }
                    Button {
                        text: row.state === "paused" || row.state === "failed" ? qsTr("Reprendre") : qsTr("Pause")
                        enabled: ["queued", "active", "retry_wait", "paused", "failed"].includes(row.state)
                        Accessible.name: (row.state === "paused" || row.state === "failed"
                                          ? qsTr("Reprendre %1") : qsTr("Mettre en pause %1")).arg(row.name)
                        onClicked: row.state === "paused" || row.state === "failed"
                                   ? root.client.resume(row.taskId) : root.client.pause(row.taskId)
                    }
                    Button {
                        text: qsTr("Annuler")
                        enabled: !["completed", "cancelled"].includes(row.state)
                        Accessible.name: qsTr("Annuler %1").arg(row.name)
                        onClicked: root.client.cancel(row.taskId)
                    }
                }
            }
        }

        Label {
            text: root.client.connected ? qsTr("%n tâche(s)", "", list.count) : qsTr("Moteur non connecté")
            opacity: 0.8
        }
    }
}
