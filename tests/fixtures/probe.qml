// Test probe: a full-screen window that prints the input it receives as
// JSON lines and draws a red 9x9 marker centered on each click. In the
// bottom right corner, controls for the accessibility tests: a button, a
// name field and a password field.
// Run: /usr/lib/qt6/bin/qml probe.qml
import QtQuick
import QtQuick.Controls
import QtQuick.Window

Window {
    id: win
    title: "deskctl-probe"
    visible: true
    visibility: Window.FullScreen
    color: "#000000"

    function emit(o) { console.log(JSON.stringify(o)) }

    onWidthChanged: if (width > 0 && height > 0 && active) emit({ev: "size", w: width, h: height})
    onActiveChanged: if (active) emit({ev: "ready", w: width, h: height, dci: Qt.styleHints.mouseDoubleClickInterval})

    ListModel { id: marks }

    Repeater {
        model: marks
        Rectangle {
            x: model.x - 4
            y: model.y - 4
            width: 9
            height: 9
            color: "#ff0000"
        }
    }

    MouseArea {
        anchors.fill: parent
        hoverEnabled: true
        acceptedButtons: Qt.AllButtons
        onPositionChanged: m => win.emit({ev: "move", x: m.x, y: m.y})
        onPressed: m => win.emit({ev: "press", button: m.button, x: m.x, y: m.y})
        onReleased: m => win.emit({ev: "release", button: m.button, x: m.x, y: m.y})
        onClicked: m => {
            marks.append({x: m.x, y: m.y});
            win.emit({ev: "click", button: m.button, x: m.x, y: m.y});
        }
        onDoubleClicked: m => win.emit({ev: "double", button: m.button, x: m.x, y: m.y})
        onWheel: w => win.emit({ev: "wheel", dx: w.angleDelta.x, dy: w.angleDelta.y})
    }

    // TextEdit, not TextInput: it takes Tab and Return as text.
    TextEdit {
        id: input
        focus: true
        anchors.centerIn: parent
        width: parent.width / 2
        color: "#ffffff"
        font.pixelSize: 24
        // Long texts: length only, so the log stays small.
        onTextChanged: win.emit({ev: "text", len: text.length, text: text.length <= 200 ? text : ""})
        Keys.onPressed: e => {
            win.emit({ev: "key", key: e.key, mods: e.modifiers, text: e.text});
            e.accepted = false;
        }
    }

    Column {
        anchors.right: parent.right
        anchors.bottom: parent.bottom
        anchors.margins: 20
        spacing: 8

        Button {
            text: "Increment"
            onClicked: win.emit({ev: "button", name: text})
        }
        TextField {
            Accessible.name: "Name"
            placeholderText: "Name"
            onTextChanged: win.emit({ev: "field", name: "Name", text: text})
            onActiveFocusChanged: win.emit({ev: "focus", name: "Name", focused: activeFocus})
        }
        TextField {
            Accessible.name: "Password"
            echoMode: TextInput.Password
            onTextChanged: win.emit({ev: "field", name: "Password", len: text.length})
        }
    }
}
