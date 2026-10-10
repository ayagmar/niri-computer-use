// Qt Quick fixture for the accessibility checks, run with `qml6 qt.qml -- <TEST_DIR>`
// inside the harness's private session. A 400x300 window with a label, a button that
// counts its activations in `TEST_DIR/qt-count`, and a text field. The count is written
// with an XMLHttpRequest PUT to a file URL, which Qt allows only with
// QML_XHR_ALLOW_FILE_WRITE=1, set for this process alone.
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

ApplicationWindow {
    width: 400
    height: 300
    visible: true
    title: "qt fixture"
    property int clicks: 0
    property string root: Qt.application.arguments[Qt.application.arguments.length - 1]

    function report(text) {
        const request = new XMLHttpRequest()
        request.open("PUT", "file://" + root + "/qt-count")
        request.send(text)
    }

    ColumnLayout {
        anchors.fill: parent
        anchors.margins: 20
        spacing: 12
        Label { text: "Qt fixture" }
        Button {
            text: "Qt: " + clicks
            onClicked: {
                clicks += 1
                report(String(clicks))
            }
        }
        TextField { placeholderText: "qt entry" }
    }

    Component.onCompleted: report("0")
    Timer { interval: 240000; running: true; onTriggered: Qt.quit() }
}
