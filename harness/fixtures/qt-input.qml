// Qt Quick text field for the input checks, run with `qml6 qt-input.qml -- <TEST_DIR>`
// inside the harness's private session: one focused field filling a 400x300 window, which
// reports its text in `qt-input-text`. The file is written as in `qt.qml`, with an
// XMLHttpRequest PUT that Qt allows only with QML_XHR_ALLOW_FILE_WRITE=1, set for this
// process alone.
import QtQuick
import QtQuick.Controls

ApplicationWindow {
    width: 400
    height: 300
    visible: true
    title: "qt input"
    property string root: Qt.application.arguments[Qt.application.arguments.length - 1]

    function report(text) {
        const request = new XMLHttpRequest()
        request.open("PUT", "file://" + root + "/qt-input-text")
        request.send(text)
    }

    TextField {
        anchors.fill: parent
        anchors.margins: 20
        focus: true
        onTextChanged: report(text)
    }

    Component.onCompleted: report("")
    Timer { interval: 240000; running: true; onTriggered: Qt.quit() }
}
