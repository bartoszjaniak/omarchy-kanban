import QtQuick
import QtQuick.Controls
import Quickshell.Io
import qs.Commons
import qs.Ui

Panel {
  id: root
  moduleName: "bart.kanban"
  manageIpc: false
  implicitWidth: button.implicitWidth
  implicitHeight: button.implicitHeight

  property var tasks: []
  property var drafts: []
  property string error: ""
  property string source: ""
  property string lastSync: ""
  property bool ready: false
  property bool connected: false
  property bool showDrafts: false
  property var unconfirmed: null
  readonly property int remaining: tasks.filter(function(t) { return t.status !== "done" && t.status !== "cancelled" }).length
  readonly property var lanes: ["backlog", "todo", "running", "waiting", "error", "done"]

  function send(command) {
    if (backend.running && ready) backend.write(JSON.stringify(command) + "\n")
  }

  Process {
    id: backend
    command: [Qt.resolvedUrl("kanban-backend").toString().replace(/^file:\/\//, ""), "bridge"]
    stdinEnabled: true
    running: true
    stdout: SplitParser {
      onRead: function(line) {
        try {
          var state = JSON.parse(line)
          root.tasks = state.tasks || []
          root.drafts = state.drafts || []
          root.connected = state.connected || false
          root.source = state.source || ""
          root.lastSync = state.last_sync ? new Date(state.last_sync * 1000).toLocaleTimeString() : "—"
          root.error = state.error || ""
          root.unconfirmed = state.unconfirmed || null
          root.ready = true
        } catch (e) { root.error = "Błąd odpowiedzi klienta: " + e }
      }
    }
    onExited: {
      root.ready = false
      root.connected = false
      root.error = "Klient zakończył pracę. Kliknij Połącz ponownie."
    }
  }

  BarIconButton {
    id: button
    anchors.fill: parent
    bar: root.bar
    slotSize: Style.space(64)
    opticalSize: Style.space(58)
    text: "▥ " + root.remaining + (root.connected ? "" : " !")
    tooltipText: "Agent Kanban · " + (root.connected ? root.source : "offline / cache")
    onPressed: function(b) { if (b === Qt.LeftButton) root.toggle() }
  }

  KeyboardPanel {
    id: popup
    anchorItem: button
    owner: root
    bar: root.bar
    open: root.opened
    centerOnBar: true
    contentWidth: fittedContentWidth(Style.space(1000))
    contentHeight: cappedContentHeight(Style.space(540))
    focusTarget: content

    Item {
      id: content
      anchors.fill: parent
      focus: true
      Keys.onEscapePressed: root.close()
      Column {
        anchors.fill: parent
        spacing: Style.space(12)
        Row {
          spacing: Style.space(12)
          Text { text: "AGENT·KANBAN"; color: Color.foreground; font.pixelSize: Style.space(20); font.bold: true }
          Text {
            text: (root.connected ? "ONLINE · " : "OFFLINE · ") + root.source + " · " + root.lastSync
            color: root.connected ? "#9ece6a" : "#e0af68"
            font.pixelSize: Style.space(11)
            anchors.verticalCenter: parent.verticalCenter
          }
        }
        Row {
          width: parent.width
          spacing: Style.space(8)
          Button { text: root.showDrafts ? "Tablica" : "Szkice · " + root.drafts.length; onClicked: root.showDrafts = !root.showDrafts }
          Button { text: "Pełny widok ↗"; enabled: root.connected; onClicked: root.send({action: "open"}) }
          Button {
            text: root.ready ? "Odśwież" : "Połącz ponownie"
            onClicked: { if (backend.running) root.send({action: "refresh"}); else backend.running = true }
          }
          Button { text: "Sprawdź niepotwierdzoną komendę"; visible: root.unconfirmed !== null; enabled: root.connected; onClicked: root.send({action: "retry_unconfirmed"}) }
        }
        Row {
          width: parent.width
          spacing: Style.space(8)
          TextField {
            id: titleInput
            width: parent.width - addButton.width - parent.spacing
            height: Style.space(36)
            placeholderText: "Nowy pomysł → lokalny szkic (także offline)…"
            maximumLength: 160
            enabled: root.ready
            color: Color.foreground
            placeholderTextColor: Qt.alpha(Color.foreground, 0.45)
            background: Rectangle { color: Qt.alpha(Color.foreground, 0.06); radius: Style.space(5); border.color: Qt.alpha(Color.foreground, 0.18) }
            function submit() {
              if (text.trim() !== "") { root.send({action: "add", title: text}); text = ""; root.showDrafts = true }
            }
            onAccepted: submit()
          }
          Button { id: addButton; text: "+ Szkic"; enabled: root.ready && titleInput.text.trim() !== ""; onClicked: titleInput.submit() }
        }
        Item {
          width: parent.width
          height: parent.height - y - footer.implicitHeight - Style.space(12)
          Flickable {
            anchors.fill: parent
            visible: !root.showDrafts
            clip: true
            contentWidth: lanesRow.width
            contentHeight: height
            flickableDirection: Flickable.HorizontalFlick
            ScrollBar.horizontal: ScrollBar { }
            Row {
              id: lanesRow
              height: parent.height - Style.space(12)
              spacing: Style.space(10)
              Repeater {
                model: ["Backlog", "TODO", "W toku", "Czeka na Ciebie", "Błąd", "DONE"]
                delegate: Rectangle {
                  id: lane
                  required property int index
                  required property string modelData
                  readonly property var laneTasks: root.tasks.filter(function(t) { return t.status === root.lanes[lane.index] || (lane.index === 2 && t.status === "cancelling") })
                  width: Style.space(210)
                  height: lanesRow.height
                  radius: Style.space(7)
                  color: Qt.alpha(Color.foreground, 0.035)
                  border.color: Qt.alpha(Color.foreground, 0.10)
                  Column {
                    anchors.fill: parent
                    anchors.margins: Style.space(9)
                    spacing: Style.space(9)
                    Text { text: lane.modelData + " · " + lane.laneTasks.length; color: ["#a9a9b2", "#e0af68", "#7aa2f7", "#bb9af7", "#f7768e", "#9ece6a"][lane.index]; font.pixelSize: Style.space(12); font.bold: true }
                    ListView {
                      width: parent.width
                      height: parent.height - y
                      clip: true
                      spacing: Style.space(8)
                      model: lane.laneTasks
                      delegate: Rectangle {
                        required property var modelData
                        width: ListView.view.width
                        height: cardContent.implicitHeight + Style.space(16)
                        radius: Style.space(5)
                        color: Qt.alpha(Color.foreground, 0.07)
                        Column {
                          id: cardContent
                          anchors.left: parent.left; anchors.right: parent.right; anchors.top: parent.top
                          anchors.margins: Style.space(8)
                          spacing: Style.space(6)
                          Text { text: modelData.agent + " · " + modelData.attempts + "/" + modelData.max_attempts; color: Color.foreground; opacity: 0.5; font.pixelSize: Style.space(10) }
                          Text { width: parent.width; text: modelData.title; textFormat: Text.PlainText; wrapMode: Text.Wrap; color: Color.foreground; font.pixelSize: Style.space(12) }
                          Text { width: parent.width; text: modelData.error || (modelData.pending ? "Wymagana decyzja → pełny widok" : ""); textFormat: Text.PlainText; visible: text !== ""; maximumLineCount: 3; elide: Text.ElideRight; wrapMode: Text.Wrap; color: "#e0af68"; font.pixelSize: Style.space(10) }
                          Row {
                            spacing: Style.space(4)
                            Button {
                              text: "TODO →"
                              visible: modelData.status === "backlog"
                              enabled: root.connected
                              onClicked: root.send({action: "ready", id: modelData.id, revision: modelData.revision})
                            }
                            Button {
                              text: "Otwórz ↗"
                              enabled: root.connected
                              onClicked: root.send({action: "open", id: modelData.id})
                            }
                          }
                        }
                      }
                    }
                  }
                }
              }
            }
          }
          ListView {
            anchors.fill: parent
            visible: root.showDrafts
            clip: true
            spacing: Style.space(8)
            model: root.drafts
            delegate: Rectangle {
              required property var modelData
              width: ListView.view.width
              height: Style.space(72)
              radius: Style.space(5)
              color: Qt.alpha(Color.foreground, 0.06)
              Column {
                anchors.fill: parent; anchors.margins: Style.space(8); spacing: Style.space(4)
                Text { text: modelData.command.task.title; textFormat: Text.PlainText; color: Color.foreground; font.pixelSize: Style.space(12) }
                Button { text: "Wyślij do Backlogu Core"; enabled: root.connected; onClicked: root.send({action: "send_draft", id: modelData.id}) }
              }
            }
          }
        }
        Text {
          id: footer
          width: parent.width
          text: root.error || "Trwałe dane przy Core · konfiguracja, rozmowy i decyzje w pełnym widoku · przewiń tablicę w poziomie"
          textFormat: Text.PlainText
          color: Color.foreground
          opacity: root.error ? 1 : 0.5
          font.pixelSize: Style.space(11)
          wrapMode: Text.Wrap
        }
      }
    }
  }
}
