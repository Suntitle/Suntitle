import sys
from PyQt5.QtWidgets import QApplication, QWidget, QPushButton, QColorDialog, QVBoxLayout, QFrame, QLabel, QMainWindow
from PyQt5.QtGui import QColor
from PyQt5.QtCore import Qt, QThread, pyqtSignal
import websocket
import threading
import time

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
        self.panel = QFrame(self)
        self.panel.setFrameShape(QFrame.StyledPanel)
        # RGBA: black with 50% opacity (alpha=128 out of 255)
        self.panel.setStyleSheet("background-color: rgba(0, 0, 0, 128);")
        self.panel.setFixedHeight(150)
        self.panel.setFixedWidth(300)
        self.label_1 = QLabel("Waiting for WebSocket messages...", self.panel)
        self.label_1.setStyleSheet("color: white; font-size: 18px;")
        # moving position
        self.label_1.move(0, 0)

        self.label_1.adjustSize()

        self.start_websocket()

        self.old_pos = None # For dragging

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
        ws_url = "ws://localhost:8765/"  # Replace with your own server if needed
        self.ws_thread = WebSocketClientThread(ws_url)
        self.ws_thread.message_received.connect(self.display_message)
        self.ws_thread.start()

    def display_message(self, msg):
        self.label_1.setText(msg)

    def closeEvent(self, event):
        if self.ws_thread:
            self.ws_thread.stop()
            self.ws_thread.wait()
        event.accept()

app = QApplication(sys.argv)
mywindow = mymainwindow()
mywindow.show()
app.exec_()
