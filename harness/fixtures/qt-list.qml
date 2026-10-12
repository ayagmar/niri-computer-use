// Qt Quick list fixture for the accessibility checks, run with
// `qml6 qt-list.qml -- <TEST_DIR>` inside the harness's private session. A 400x300 window
// with `Step` and `Jump` buttons over three lists that reuse their rows' items for other
// records (`reuseItems`), each row a label and a `Delete` button that removes the record
// it shows then and appends it to `qt-list-deletes`. The first list's rows are plain
// layouts, which Qt leaves off the accessibility bus, so their label and button sit
// straight in the window's panel or in a filler; the second's are unnamed list items, as
// GTK 4's are; the third's are list items named after their record. Records are
// synthetic: `Record 001` to `Record 200`, `Item 001` to `Item 200` and `Named 001` to
// `Named 200`. `Step` scrolls every list 120 pixels down and `Jump` brings each one's
// 101st record into view. The file is written with an XMLHttpRequest PUT to a file URL,
// which Qt allows only with QML_XHR_ALLOW_FILE_WRITE=1, set for this process alone.
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

ApplicationWindow {
    width: 400
    height: 300
    visible: true
    title: "qt list fixture"
    property string root: Qt.application.arguments[Qt.application.arguments.length - 1]
    property var deletes: []

    function report(name, text) {
        const request = new XMLHttpRequest()
        request.open("PUT", "file://" + root + "/" + name)
        request.send(text)
    }

    // A press on a row that shows no record, such as one the list holds for reuse, is
    // reported as such, so the checks see it went to the wrong row.
    function deleted(records, index) {
        if (index < 0 || index >= records.count) {
            deletes.push("a row with no record")
        } else {
            deletes.push(records.get(index).record)
            records.remove(index)
        }
        report("qt-list-deletes", JSON.stringify(deletes))
    }

    function filled(records, prefix) {
        for (let n = 1; n <= 200; n++)
            records.append({ record: prefix + " " + String(n).padStart(3, "0") })
    }

    ListModel { id: plainRecords }
    ListModel { id: itemRecords }
    ListModel { id: namedRecords }

    ColumnLayout {
        anchors.fill: parent
        anchors.margins: 4
        RowLayout {
            Button {
                text: "Step"
                Layout.fillWidth: true
                onClicked: {
                    for (const list of [plain, items, named])
                        list.contentY += 120
                }
            }
            Button {
                text: "Jump"
                Layout.fillWidth: true
                onClicked: {
                    for (const list of [plain, items, named])
                        list.positionViewAtIndex(100, ListView.Beginning)
                }
            }
        }
        RowLayout {
            Layout.fillWidth: true
            Layout.fillHeight: true
            spacing: 2
            ListView {
                id: plain
                Layout.fillWidth: true
                Layout.fillHeight: true
                clip: true
                reuseItems: true
                model: plainRecords
                delegate: RowLayout {
                    id: row
                    required property string record
                    required property int index
                    width: ListView.view.width
                    spacing: 2
                    Label { text: row.record; Layout.fillWidth: true }
                    Button { text: "Delete"; onClicked: deleted(plainRecords, row.index) }
                }
            }
            ListView {
                id: items
                Layout.fillWidth: true
                Layout.fillHeight: true
                clip: true
                reuseItems: true
                model: itemRecords
                delegate: Item {
                    id: item
                    required property string record
                    required property int index
                    Accessible.role: Accessible.ListItem
                    width: ListView.view.width
                    implicitHeight: itemLayout.implicitHeight
                    RowLayout {
                        id: itemLayout
                        anchors.fill: parent
                        spacing: 2
                        Label { text: item.record; Layout.fillWidth: true }
                        Button { text: "Delete"; onClicked: deleted(itemRecords, item.index) }
                    }
                }
            }
            ListView {
                id: named
                Layout.fillWidth: true
                Layout.fillHeight: true
                clip: true
                reuseItems: true
                model: namedRecords
                delegate: Item {
                    id: namedItem
                    required property string record
                    required property int index
                    Accessible.role: Accessible.ListItem
                    Accessible.name: record
                    width: ListView.view.width
                    implicitHeight: namedLayout.implicitHeight
                    RowLayout {
                        id: namedLayout
                        anchors.fill: parent
                        spacing: 2
                        Label { text: namedItem.record; Layout.fillWidth: true }
                        Button { text: "Delete"; onClicked: deleted(namedRecords, namedItem.index) }
                    }
                }
            }
        }
    }

    Component.onCompleted: {
        filled(plainRecords, "Record")
        filled(itemRecords, "Item")
        filled(namedRecords, "Named")
        report("qt-list-deletes", "[]")
    }
    Timer { interval: 240000; running: true; onTriggered: Qt.quit() }
}
