import sys
from PyQt5.QtWidgets import QApplication, QWidget, QPushButton, QColorDialog, QVBoxLayout, QFrame, QLabel, QMainWindow, QSizePolicy
from PyQt5.QtGui import QColor
from PyQt5.QtCore import Qt, QThread, pyqtSignal
import websocket
import threading
import time
import argparse

# Worker thread for WebSocket connection
class WebSocketClientThread(QThread):
    message_received = pyqtSignal(str)

    def __init__(self, ws_url):
        super().__init__()
        self.ws_url = ws_url
        self.ws = None
        self.running = True

    def run(self):
        def on_message(ws, message):
            self.message_received.emit(message)

        def on_error(ws, error):
            self.message_received.emit(f"WebSocket error: {error}")

        def on_close(ws, close_status_code, close_msg):
            self.message_received.emit("WebSocket closed.")

        def on_open(ws):
            self.message_received.emit("WebSocket connected.")

        self.ws = websocket.WebSocketApp(
            self.ws_url,
            on_message=on_message,
            on_error=on_error,
            on_close=on_close,
            on_open=on_open,
        )

        # Run websocket in this thread
        while self.running:
            self.ws.run_forever()
            time.sleep(1)

    def stop(self):
        self.running = False
        if self.ws:
            self.ws.close()


class mymainwindow(QMainWindow):
    def __init__(self):
        QMainWindow.__init__(self, None, Qt.WindowStaysOnTopHint)
        self.setWindowFlags(self.windowFlags() | Qt.FramelessWindowHint)
        self.setAttribute(Qt.WA_TranslucentBackground)
        screen_width = QApplication.desktop().screenGeometry().width()
        self.width = int(screen_width * 0.8)
        self.height = 75
        self.panel = QFrame(self)
        self.panel.setFrameShape(QFrame.StyledPanel)
        # RGBA: black with 50% opacity (alpha=128 out of 255)
        self.panel.setStyleSheet("background-color: rgba(0, 0, 0, 128);")
        self.panel.setFixedHeight(75)
        self.panel.setFixedWidth(int(screen_width * 0.8))
        # self.panel.setFixedWidth(1000)
        self.label_1 = QLabel("Waiting for WebSocket messages...", self.panel)
        self.label_1.setStyleSheet("color: white; font-size: 30px;")
        self.label_1.setWordWrap(True)
        self.label_1.setAlignment(Qt.AlignLeft | Qt.AlignTop) # Align text to top-left
        self.label_1.setFixedWidth(self.panel.width())
        self.label_1.setFixedHeight(75) # Approximately 2 lines of 18px font with some padding
        # moving position
        self.label_1.move(0, 0)

        self.start_websocket()

        self.old_pos = None # For dragging
        self.setGeometry(int(screen_width * 0.1), 50, self.width, self.height)

    def mousePressEvent(self, event):
        if event.button() == Qt.LeftButton:
            self.old_pos = event.globalPos()

    def mouseReleaseEvent(self, event):
        if event.button() == Qt.LeftButton:
            self.old_pos = None

    def mouseMoveEvent(self, event):
        if not self.old_pos:
            return
        delta = event.globalPos() - self.old_pos
        self.move(self.pos() + delta)
        self.old_pos = event.globalPos()

    def keyPressEvent(self, event):
        if event.key() == Qt.Key_Escape: 
            self.close()
        else:
            super().keyPressEvent(event)
    
    def start_websocket(self):
        ws_url = f"ws://{args.host}/display"  # Replace with your own server if needed
        self.ws_thread = WebSocketClientThread(ws_url)
        self.ws_thread.message_received.connect(self.display_message)
        self.ws_thread.start()

    def display_message(self, msg):
        if not hasattr(self, '_message_buffer'):
            self._message_buffer = []
        
        # Clear initial "Waiting for WebSocket messages..." if it's the first actual message
        if len(self._message_buffer) == 0 and self.label_1.text() == "Waiting for WebSocket messages...":
            self.label_1.setText("") # Clear the initial message visually
        
        if args.replace_on_receive:
            # Discard the previous flow and display only the newest message.
            self._message_buffer = [msg]
        else:
            self._message_buffer.append(msg)

        # Loop to ensure the displayed text fits within two lines, by removing oldest messages
        while True:
            if not self._message_buffer:
                text_to_display = ""
            else:
                # Join messages with a space. QLabel's word wrap will handle visual line breaks.
                text_to_display = " ".join(self._message_buffer)
            
            self.label_1.setText(text_to_display)
            font_metrics = self.label_1.fontMetrics()
            label_width = self.label_1.width()
            characters_per_line = int(label_width / font_metrics.averageCharWidth()) - 4
            
            # Calculate the bounding rectangle for the text with word wrap
            # This gives the height the text would occupy if rendered
            rect = font_metrics.boundingRect(
                0, 0, label_width, 0,  # x, y, width, height (height=0 means unlimited)
                Qt.TextWordWrap,
                text_to_display
            )

            if not self._message_buffer:
                break
            
            # Check if the text exceeds the fixed height (45px for 2 lines)
            # AND if there's more than one message in the buffer to remove.
            # If there's only one message, even if it's long, we should still display it.
            if rect.height() > self.label_1.height() and len(self._message_buffer) > 1:
                characters_per_line_temp = characters_per_line
                message_buffer_temp = self._message_buffer.copy()
                for i in range(len(self._message_buffer)):
                    if characters_per_line_temp == 0:
                        break
                    if len(self._message_buffer[i]) <= characters_per_line_temp:
                        message_buffer_temp[i] = ""
                        characters_per_line_temp -= len(self._message_buffer[i])
                    else:
                        message_buffer_temp[i] = message_buffer_temp[i][characters_per_line_temp - 1:]
                        characters_per_line_temp = 0
                self._message_buffer = [m for m in message_buffer_temp if m]
                # text_to_display = text_to_display[characters_per_line:]
            else:
                # Either the text fits within two lines, or it's a single message
                # (even if it's long and wraps to more than two lines, it should be displayed).
                break

    def closeEvent(self, event):
        if self.ws_thread:
            self.ws_thread.stop()
            self.ws_thread.wait()
        event.accept()

parser = argparse.ArgumentParser(description="Websocket client for subtitle translation.")
parser.add_argument("--host", default="localhost:3030", help="Websocket server host (e.g., localhost:3030)")
parser.add_argument(
    "--replace-on-receive",
    action="store_true",
    help="completely replace the displayed text with each new WebSocket message",
)
args = parser.parse_args()

app = QApplication(sys.argv)
mywindow = mymainwindow()
mywindow.show()
app.exec_()
