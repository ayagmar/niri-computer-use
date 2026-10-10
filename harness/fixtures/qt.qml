// Qt Quick fixture for the accessibility checks, run with `qml6 qt.qml -- <TEST_DIR>`
// inside the harness's private session. A 400x300 window with a label, a button that
// counts its activations in `TEST_DIR/qt-count`, a `Dismiss` button that closes the
// window, the `Plain entry` text field, which reports its text in `qt-entry-text`, and
// the `Password entry` field, which hides its text and counts its changes in
// `qt-password-changes` without the text. The files are written with an XMLHttpRequest
// PUT to a file URL, which Qt allows only with QML_XHR_ALLOW_FILE_WRITE=1, set for this
// process alone.
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

ApplicationWindow {
    width: 400
    height: 300
    visible: true
    title: "qt fixture"
    property int clicks: 0
    property int passwordChanges: 0
    property string root: Qt.application.arguments[Qt.application.arguments.length - 1]

    function report(name, text) {
        const request = new XMLHttpRequest()
        request.open("PUT", "file://" + root + "/" + name)
        request.send(text)
    }

    GridLayout {
        anchors.fill: parent
        anchors.margins: 20
        columns: 2
        rowSpacing: 12
        columnSpacing: 12
        Label { text: "Qt fixture"; Layout.columnSpan: 2 }
        Button {
            text: "Qt: " + clicks
            onClicked: {
                clicks += 1
                report("qt-count", String(clicks))
            }
        }
        Button {
            text: "Dismiss"
            onClicked: close()
        }
        TextField {
            Accessible.name: "Plain entry"
            onTextChanged: report("qt-entry-text", text)
        }
        TextField {
            Accessible.name: "Password entry"
            echoMode: TextInput.Password
            onTextChanged: {
                passwordChanges += 1
                report("qt-password-changes", String(passwordChanges))
            }
        }
    }

    Component.onCompleted: {
        report("qt-count", "0")
        report("qt-entry-text", "")
        report("qt-password-changes", "0")
    }
    Timer { interval: 240000; running: true; onTriggered: Qt.quit() }
}
