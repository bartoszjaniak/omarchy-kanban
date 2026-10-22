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
  property var counts: ({ todo: 0, doing: 0, done: 0, archived: 0 })
  property bool connected: false
  property string error: ""

  // Hover on a column: show the "add" card at the bottom of that column.
  property string hoverStatus: ""
  // Id of the card currently hovered; hides the "add" card while over a task.
  property string hoveredCardId: ""

  // Active drag: a floating ghost follows the cursor; the real card never moves.
  property bool dragging: false
  property var dragTask: null
  property real dragX: 0
  property real dragY: 0
  property string dragTarget: ""
  property int dragIndex: 0

  // Editor overlay.
  property var editing: null
  property string editMode: "edit"
  property string editStatus: ""

  readonly property var columns: ["todo", "doing", "done"]
  readonly property var labels: ["TODO", "IN PROGRESS", "DONE"]
  // Ikona paska zależna od stanu tablicy (glify Nerd Font).
  readonly property string iconIdle: "\uDB85\uDCF5"    // nic pilnego
  readonly property string iconDoing: "\uDB85\uDCE7"   // coś w In Progress
  readonly property string iconAllDone: "\uDB86\uDE0B" // wszystko zrobione, nic w TODO
  readonly property string icon: (counts.doing || 0) > 0 ? iconDoing
    : ((counts.todo || 0) === 0 && (counts.done || 0) > 0 ? iconAllDone : iconIdle)

  // Motywacyjny cytat dnia: lokalna lista PL, kolejność bez powtórzeń w cyklu.
  property string quoteOfDay: ""
  readonly property var quotes: [
    "Start now — the rest will follow.",
    "Small steps still count as progress.",
    "Do today what tomorrow-you will thank you for.",
    "Progress, not perfection.",
    "One task at a time.",
    "Starting is the hardest part.",
    "Done is better than perfect.",
    "A little further every day.",
    "Focus on what matters.",
    "Energy flows where attention goes.",
    "Close one loop to open the next calmly.",
    "Small step today, big leap tomorrow.",
    "You don't have to be fast — you have to be persistent.",
    "The best moment is now.",
    "Per aspera ad astra — through hardship to the stars.",
    "Festina lente — make haste slowly.",
    "Where there's a will, there's a way.",
    "Dripping water hollows out stone.",
    "Rome wasn't built in a day.",
    "One thing at a time, done well.",
    "Move, and the path appears.",
    "Better to do and regret than regret not doing.",
    "Hard work beats talent when talent doesn't work hard.",
    "Success is the sum of small efforts repeated day in and day out.",
    "Discipline is choosing between what you want now and what you want most.",
    "Motivation gets you started; habit keeps you going.",
    "Don't wait for inspiration — begin and it will come.",
    "Every master was once a beginner.",
    "Do a little, but do it often.",
    "Focus is the new superpower.",
    "One thing, done well.",
    "A plan without action is just a dream.",
    "Start where you are, use what you have.",
    "Compare yourself with yesterday's you.",
    "Persistence matters more than talent.",
    "A written task is half done.",
    "Check it off; don't put it off.",
    "Do the hard thing before it becomes urgent.",
    "Small discipline today, big freedom tomorrow.",
    "Choose direction over speed.",
    "Every day is a fresh start.",
    "Rest is part of the work.",
    "Respect your time — start on time.",
    "Not all at once — everything bit by bit.",
    "Progress rewards the patient.",
    "A bad decision beats no decision.",
    "Instead of \"no time\", say \"not a priority\".",
    "Hard things get easier once you start.",
    "Say \"no\" to what distracts you.",
    "Less, but better.",
    "Trust the process.",
    "Results are the consequence of consistency.",
    "Do the things you can't skip.",
    "Perfection doesn't exist — progress does.",
    "Begin with the most important thing.",
    "Action builds energy, waiting drains it.",
    "Consistency beats intensity.",
    "Want results? Change the routine.",
    "Haste ruins the work.",
    "Calm mind, steady hands.",
    "Be 1% better today.",
    "You don't need the whole path — just the first step.",
    "Execution is the best answer to doubt.",
    "Time flies — choose deliberately.",
    "First make it work, then make it better.",
    "Your attention is your fuel.",
    "Start with \"good\", not \"perfect\".",
    "Every finished task adds wings.",
    "Rest after work tastes best.",
    "Don't wait for Monday — start today.",
    "Strength lives in routine.",
    "Do big things in small pieces.",
    "A simple list saves the day.",
    "Close your open loops.",
    "Intentions aren't results.",
    "Hold on one more day.",
    "Only the start is hard.",
    "Change begins with a decision.",
    "Order around you, calm within you.",
    "Don't put off what matters to \"someday\".",
    "Small wins build courage.",
    "Focus on what you can change.",
    "Work smart, not just hard.",
    "Take breaks so you can run farther.",
    "Good habits are quiet support.",
    "Every step forward is a good one.",
    "You don't have to be ready — just willing.",
    "A fall is not the end, it's a lesson.",
    "Take notes — memory is unreliable.",
    "Priority is one word: \"first\".",
    "Instead of waiting — begin.",
    "Time you don't plan, someone will take.",
    "Start with yourself.",
    "Success favors the prepared.",
    "Don't count hours — count finished work.",
    "Change \"I must\" into \"I choose\".",
    "Calm is also a form of action.",
    "Be your own best support.",
    "A day without a plan has no direction.",
    "Do it consistently.",
    "Skip perfection — start and finish.",
    "Every \"not today\" moves the goal away.",
    "Walk toward what fascinates you.",
    "Don't waste time on regret — move on.",
    "Progress requires a bit of discomfort.",
    "Motivation is a myth — habits count.",
    "Start from the end: what must get done?",
    "Break the big into the small.",
    "Action cures fear.",
    "Do your thing, calmly.",
    "Concentration: one thing, right now.",
    "Win the morning, win the day.",
    "Don't boast of plans; show results.",
    "Yesterday won't return — today is still yours.",
    "Be patient with yourself.",
    "Every minute matters when you know why.",
    "Do today the one thing you avoid.",
    "Do it with heart.",
    "Focus builds freedom.",
    "A hard task won't disappear on its own.",
    "Turn excuses into steps.",
    "You win when you don't quit.",
    "Act as if you're already where you want to be.",
    "First the step, then the faith.",
    "Patience is a form of courage.",
    "\"Tomorrow\" doesn't work — \"right now\" does.",
    "A small start beats big intentions.",
    "Each day is a brick, not the whole wall.",
    "Focus: less, but deeper.",
    "Choose progress over excuses.",
    "It's not about time — it's about priorities.",
    "Plan rest like a task.",
    "You don't have to do everything — just the most important.",
    "Be persistent, not just fired up.",
    "Do it badly, just do it.",
    "No plan is a plan to fail.",
    "Every obstacle teaches.",
    "Willpower is a muscle — train it.",
    "Don't wait for a better moment — create it.",
    "Begin before you feel ready.",
    "Progress delights more than perfection.",
    "Don't count on inspiration — count on a schedule.",
    "Hard today, easy tomorrow.",
    "Be where the work is, not where it's comfortable.",
    "Take small steps, but don't stand still.",
    "Close the day with one \"done\".",
    "Victory is daily, not fireworks.",
    "Don't explain — act.",
    "Simplicity is the ultimate sophistication.",
    "Start. Finish. Repeat.",
  ]

  // Deterministyczna permutacja na cykl: w obrębie `quotes.length` dni bez powtórzeń.
  function lcg(seed) {
    var state = seed % 2147483647
    if (state <= 0) state += 2147483646
    return function() {
      state = state * 16807 % 2147483647
      return (state - 1) / 2147483646
    }
  }
  function quoteForDay(day) {
    var n = quotes.length
    var cycle = Math.floor(day / n)
    var position = day % n
    var rand = lcg(cycle + 1)
    var order = []
    for (var a = 0; a < n; a++) order.push(a)
    for (var b = n - 1; b > 0; b--) {
      var j = Math.floor(rand() * (b + 1))
      var swap = order[b]; order[b] = order[j]; order[j] = swap
    }
    return quotes[order[position]]
  }
  function pickQuote() { quoteOfDay = quoteForDay(Math.floor(Date.now() / 86400000)) }

  function send(action) { if (backend.running) backend.write(JSON.stringify(action) + "\n") }
  function rank(status) { return status === "todo" ? 0 : status === "doing" ? 1 : status === "done" ? 2 : 3 }
  function statusColor(status) { return status === "todo" ? "#a9a9b2" : status === "doing" ? "#7aa2f7" : status === "done" ? "#9ece6a" : "#5c5c66" }

  function laneByStatus(status) {
    for (var i = 0; i < laneRepeater.count; i++) {
      var lane = laneRepeater.itemAt(i)
      if (lane && lane.status === status) return lane
    }
    return null
  }

  // Column under a point given in `content` coordinates.
  function laneAt(point) {
    for (var i = 0; i < laneRepeater.count; i++) {
      var lane = laneRepeater.itemAt(i)
      if (!lane) continue
      var origin = lane.mapToItem(content, 0, 0)
      if (point.x >= origin.x && point.x <= origin.x + lane.width
          && point.y >= origin.y && point.y <= origin.y + lane.height)
        return lane.status
    }
    return ""
  }

  // Insertion index among the cards of `status`, ignoring the card being dragged.
  function indexAt(status, point, movingId) {
    var lane = laneByStatus(status)
    if (!lane) return 0
    var index = 0
    var laneTasks = lane.laneTasks
    for (var i = 0; i < laneTasks.length; i++) {
      var item = lane.cards.itemAt(i)
      if (!item) continue
      if (movingId && laneTasks[i].id === movingId) continue
      var center = item.mapToItem(content, item.width / 2, item.height / 2)
      if (point.y < center.y) return index
      index++
    }
    return index
  }

  // Float that lands between two neighbours instead of jumping to a column end.
  function insertPosition(laneTasks, index, movingId) {
    var rest = laneTasks.filter(function(t) { return t.id !== movingId })
    if (rest.length === 0) return 1
    if (index <= 0) return rest[0].position - 1
    if (index >= rest.length) return rest[rest.length - 1].position + 1
    return (rest[index - 1].position + rest[index].position) / 2
  }

  function updateDropTarget(point) {
    var status = laneAt(point)
    dragTarget = status
    dragIndex = status === "" ? 0 : indexAt(status, point, dragTask ? dragTask.id : "")
  }

  function finishDrag() {
    var task = dragTask
    var target = dragTarget
    var index = dragIndex
    dragging = false
    dragTask = null
    dragTarget = ""
    dragIndex = 0
    dragX = 0
    dragY = 0
    if (!task || target === "") return
    var lane = laneByStatus(target)
    if (!lane) return
    var position = insertPosition(lane.laneTasks, index, task.id)
    if (target !== task.status || Math.abs(position - task.position) > 0.0001)
      move(task.id, target, position)
  }

  function move(id, status, position) {
    // Optimistic: show the move, then let the bridge confirm within a couple of seconds.
    var next = tasks.slice()
    for (var i = 0; i < next.length; i++)
      if (next[i].id === id) next[i] = Object.assign({}, next[i], { status: status, position: position })
    next.sort(function(a, b) { return rank(a.status) !== rank(b.status) ? rank(a.status) - rank(b.status) : a.position - b.position })
    tasks = next
    send({ action: "move", id: id, status: status, position: position })
  }

  function openAdd(status) {
    editMode = "add"
    editStatus = status
    editing = { id: "", title: "", notes: "", status: status }
    editTitle.text = ""
    editNotes.text = ""
    editorFocus.restart()
  }
  function openEdit(task) {
    editMode = "edit"
    editStatus = task.status
    editing = task
    editTitle.text = task.title
    editNotes.text = task.notes || ""
    editorFocus.restart()
  }
  function closeEdit() { editing = null }
  function commitEdit() {
    if (!editing) return
    if (editMode === "add") send({ action: "add", title: editTitle.text, notes: editNotes.text, status: editStatus })
    else send({ action: "edit", id: editing.id, title: editTitle.text, notes: editNotes.text })
    editing = null
  }

  Timer { id: editorFocus; interval: 50; onTriggered: editTitle.forceActiveFocus() }
  // Refresh the quote when the day changes (checked hourly).
  Timer { interval: 3600000; running: true; repeat: true; onTriggered: root.pickQuote() }
  Component.onCompleted: pickQuote()

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
          root.counts = state.counts || root.counts
          root.connected = state.connected || false
          root.error = state.error || ""
        } catch (e) { root.error = "Błąd klienta: " + e }
      }
    }
    onExited: { root.connected = false; root.error = "Klient rozłączony. Uruchom ponownie usługę." }
  }

  BarIconButton {
    id: button
    anchors.fill: parent
    bar: root.bar
    slotSize: Style.space(64)
    opticalSize: Style.space(58)
    text: root.icon
    tooltipText: "Kanban · TODO " + (root.counts.todo || 0) + " · In Progress " + (root.counts.doing || 0) + " · DONE " + (root.counts.done || 0)
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
    contentHeight: cappedContentHeight(Style.space(560))
    focusTarget: content

    Item {
      id: content
      anchors.fill: parent
      focus: true
      Keys.onEscapePressed: { if (root.editing !== null) root.closeEdit(); else root.close() }

      Column {
        anchors.fill: parent
        spacing: Style.space(10)

        Item {
          width: parent.width
          height: Style.space(24)
          Rectangle {
            id: accentDot
            width: Style.space(10)
            height: width
            radius: width / 2
            color: "#ff6258"
            anchors.left: parent.left
            anchors.verticalCenter: parent.verticalCenter
          }
          Text {
            id: quoteText
            anchors.left: accentDot.right
            anchors.leftMargin: Style.space(12)
            anchors.right: statusText.left
            anchors.rightMargin: Style.space(12)
            anchors.verticalCenter: parent.verticalCenter
            text: root.quoteOfDay
            textFormat: Text.PlainText
            elide: Text.ElideRight
            color: Color.foreground
            font.pixelSize: Style.space(15)
            font.italic: true
          }
          Text {
            id: statusText
            anchors.right: parent.right
            anchors.verticalCenter: parent.verticalCenter
            text: root.connected ? "ONLINE" : "OFFLINE"
            color: root.connected ? "#9ece6a" : "#e0af68"
            font.pixelSize: Style.space(11)
          }
        }

        Row {
          id: lanes
          width: parent.width
          height: parent.height - y - footer.height - Style.space(8)
          spacing: Style.space(10)

          Repeater {
            id: laneRepeater
            model: root.columns.length
            delegate: Rectangle {
              id: lane
              required property int index
              readonly property string status: root.columns[index]
              readonly property var laneTasks: root.tasks.filter(function(t) { return t.status === lane.status })
              readonly property bool highlighted: root.dragging && root.dragTarget === lane.status
              width: (lanes.width - Style.space(20)) / 3
              height: lanes.height
              radius: Style.space(8)
              clip: true
              color: lane.highlighted ? Qt.alpha("#7aa2f7", 0.18) : Qt.alpha(Color.foreground, 0.035)
              border.width: lane.highlighted ? 2 : 1
              border.color: lane.highlighted ? "#7aa2f7" : Qt.alpha(Color.foreground, 0.10)

              property alias cards: cardRepeater

              // Hover only (does not block the cards). Shows the "add" card at the bottom.
              HoverHandler {
                id: laneHover
                onHoveredChanged: {
                  if (laneHover.hovered) root.hoverStatus = lane.status
                  else if (root.hoverStatus === lane.status) root.hoverStatus = ""
                }
              }

              Column {
                anchors.fill: parent
                anchors.margins: Style.space(9)
                spacing: Style.space(8)
                Row {
                  spacing: Style.space(6)
                  Rectangle { width: Style.space(8); height: width; radius: width / 2; color: root.statusColor(lane.status); anchors.verticalCenter: parent.verticalCenter }
                  Text {
                    text: root.labels[lane.index] + " · " + lane.laneTasks.length
                    color: root.statusColor(lane.status)
                    font.pixelSize: Style.space(12)
                    font.bold: true
                    anchors.verticalCenter: parent.verticalCenter
                  }
                }
                Column {
                  id: cardsColumn
                  width: parent.width
                  spacing: Style.space(8)
                  Repeater {
                    id: cardRepeater
                    model: lane.laneTasks
                    delegate: Rectangle {
                      id: card
                      required property var modelData
                      width: cardsColumn.width
                      height: contentColumn.implicitHeight + Style.space(16)
                      radius: Style.space(6)
                      color: Qt.alpha(Color.foreground, 0.075)
                      border.color: Qt.alpha(Color.foreground, 0.10)

                      Rectangle {
                        id: stripe
                        anchors.left: parent.left
                        anchors.top: parent.top
                        anchors.bottom: parent.bottom
                        width: Style.space(3)
                        color: root.statusColor(card.modelData.status)
                        radius: card.radius
                      }

                      Column {
                        id: contentColumn
                        anchors.left: stripe.right
                        anchors.right: details.left
                        anchors.top: parent.top
                        anchors.margins: Style.space(8)
                        spacing: Style.space(4)
                        Text {
                          width: parent.width
                          text: card.modelData.title
                          textFormat: Text.PlainText
                          wrapMode: Text.Wrap
                          color: Color.foreground
                          font.pixelSize: Style.space(12)
                        }
                        Text {
                          width: parent.width
                          visible: (card.modelData.notes || "").length > 0
                          text: card.modelData.notes
                          textFormat: Text.PlainText
                          wrapMode: Text.Wrap
                          maximumLineCount: 3
                          elide: Text.ElideRight
                          color: Color.foreground
                          opacity: 0.6
                          font.pixelSize: Style.space(10)
                        }
                      }

                      // The whole card is the drag surface. A click does nothing.
                      MouseArea {
                        id: dragArea
                        anchors.left: stripe.right
                        anchors.right: details.left
                        anchors.top: parent.top
                        anchors.bottom: parent.bottom
                        cursorShape: root.dragging ? Qt.ClosedHandCursor : Qt.OpenHandCursor
                        property bool armed: false
                        property point pressPoint
                        onPressed: {
                          armed = true
                          pressPoint = dragArea.mapToItem(content, mouse.x, mouse.y)
                        }
                        onPositionChanged: {
                          var point = dragArea.mapToItem(content, mouse.x, mouse.y)
                          if (!root.dragging && armed
                              && Math.abs(point.x - pressPoint.x) + Math.abs(point.y - pressPoint.y) > Style.space(6)) {
                            root.dragging = true
                            root.dragTask = card.modelData
                          }
                          if (root.dragging) {
                            root.dragX = point.x
                            root.dragY = point.y
                            root.updateDropTarget(point)
                          }
                        }
                        onReleased: { armed = false; if (root.dragging) root.finishDrag() }
                        onCanceled: { armed = false; if (root.dragging) root.finishDrag() }
                      }

                      // Details icon (replaces the old grab handle).
                      Item {
                        id: details
                        anchors.right: parent.right
                        anchors.top: parent.top
                        anchors.bottom: parent.bottom
                        width: Style.space(24)
                        Text {
                          anchors.centerIn: parent
                          text: "⋯"
                          color: Color.foreground
                          opacity: detailsHover.hovered ? 0.95 : 0.5
                          font.pixelSize: Style.space(16)
                        }
                        HoverHandler { id: detailsHover; cursorShape: Qt.PointingHandCursor }
                        TapHandler { onTapped: root.openEdit(card.modelData) }
                      }

                      // Remember that a card is hovered so the column's "add" card hides.
                      HoverHandler {
                        onHoveredChanged: {
                          if (hovered) root.hoveredCardId = card.modelData.id
                          else if (root.hoveredCardId === card.modelData.id) root.hoveredCardId = ""
                        }
                      }
                    }
                  }
                  Rectangle {
                    id: virtualCard
                    width: parent.width
                    height: visible ? Style.space(48) : 0
                    visible: root.hoverStatus === lane.status && !root.dragging && root.editing === null && root.hoveredCardId === ""
                    radius: Style.space(6)
                    color: Qt.alpha("#7aa2f7", 0.08)
                    border.color: Qt.alpha("#7aa2f7", 0.6)
                    border.width: 1
                    Text {
                      anchors.centerIn: parent
                      text: "＋  Nowe zadanie"
                      color: "#7aa2f7"
                      font.pixelSize: Style.space(12)
                    }
                    HoverHandler { cursorShape: Qt.PointingHandCursor }
                    TapHandler {
                      onTapped: root.openAdd(lane.status)
                    }
                  }
                }
              }
            }
          }
        }

        Text {
          id: footer
          width: parent.width
          height: visible ? implicitHeight : 0
          visible: root.error !== ""
          text: root.error
          textFormat: Text.PlainText
          color: "#e0af68"
          font.pixelSize: Style.space(11)
          wrapMode: Text.Wrap
        }
      }

      // Floating ghost that follows the cursor while dragging.
      Rectangle {
        id: ghost
        visible: root.dragging && root.dragTask !== null
        x: root.dragX - width / 2
        y: root.dragY - height / 2
        width: Style.space(200)
        height: ghostText.implicitHeight + Style.space(18)
        radius: Style.space(6)
        color: "#2a2a31"
        border.color: "#7aa2f7"
        border.width: 1
        opacity: 0.92
        z: 5
        Text {
          id: ghostText
          anchors.centerIn: parent
          width: parent.width - Style.space(16)
          text: root.dragTask ? root.dragTask.title : ""
          textFormat: Text.PlainText
          wrapMode: Text.Wrap
          horizontalAlignment: Text.AlignHCenter
          color: Color.foreground
          font.pixelSize: Style.space(12)
        }
      }

      // Inline editor overlay: open a task or create a new one.
      Item {
        id: editor
        anchors.fill: parent
        z: 10
        visible: root.editing !== null

        Rectangle {
          anchors.fill: parent
          color: Qt.rgba(0, 0, 0, 0.45)
          MouseArea { anchors.fill: parent; onClicked: root.closeEdit() }
        }

        Rectangle {
          width: Math.min(Style.space(440), parent.width - Style.space(40))
          height: editColumn.implicitHeight + Style.space(28)
          anchors.centerIn: parent
          radius: Style.space(10)
          color: "#1e1e22"
          border.color: Qt.alpha(Color.foreground, 0.22)

          Column {
            id: editColumn
            anchors.fill: parent
            anchors.margins: Style.space(14)
            spacing: Style.space(8)

            Row {
              spacing: Style.space(6)
              Rectangle { width: Style.space(9); height: width; radius: width / 2; color: root.statusColor(root.editStatus); anchors.verticalCenter: parent.verticalCenter }
              Text {
                text: root.editMode === "add" ? "Nowe zadanie · " + root.labels[root.columns.indexOf(root.editStatus)] : "Szczegóły zadania"
                color: Color.foreground; font.bold: true; font.pixelSize: Style.space(14)
                anchors.verticalCenter: parent.verticalCenter
              }
            }
            TextField {
              id: editTitle
              width: parent.width
              height: Style.space(34)
              color: Color.foreground
              placeholderText: "Nazwa zadania"
              placeholderTextColor: Qt.alpha(Color.foreground, 0.45)
              background: Rectangle { color: Qt.alpha(Color.foreground, 0.06); radius: Style.space(5); border.color: Qt.alpha(Color.foreground, 0.18) }
              onAccepted: root.commitEdit()
            }
            Rectangle {
              width: parent.width
              height: Style.space(110)
              color: Qt.alpha(Color.foreground, 0.06)
              radius: Style.space(5)
              border.color: Qt.alpha(Color.foreground, 0.18)
              ScrollView {
                anchors.fill: parent
                anchors.margins: Style.space(6)
                clip: true
                contentWidth: availableWidth
                ScrollBar.vertical.policy: ScrollBar.AsNeeded
                TextArea {
                  id: editNotes
                  width: parent.width
                  color: Color.foreground
                  placeholderText: "Notatka / szczegóły…"
                  placeholderTextColor: Qt.alpha(Color.foreground, 0.45)
                  wrapMode: TextArea.Wrap
                  background: null
                }
              }
            }
            Row {
              spacing: Style.space(8)
              anchors.right: parent.right
              Button {
                text: "Anuluj"
                anchors.verticalCenter: parent.verticalCenter
                onClicked: root.closeEdit()
                HoverHandler { cursorShape: Qt.PointingHandCursor }
              }
              Rectangle {
                height: Style.space(34)
                width: primaryLabel.implicitWidth + Style.space(30)
                radius: Style.space(6)
                color: primaryTap.pressed ? "#d6d6d6" : "white"
                anchors.verticalCenter: parent.verticalCenter
                Text {
                  id: primaryLabel
                  anchors.centerIn: parent
                  text: root.editMode === "add" ? "Dodaj" : "Zapisz"
                  color: "black"
                  font.bold: true
                  font.pixelSize: Style.space(12)
                }
                HoverHandler { cursorShape: Qt.PointingHandCursor }
                TapHandler { id: primaryTap; onTapped: root.commitEdit() }
              }
            }
          }
        }
      }
    }
  }
}
