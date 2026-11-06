use cxx_qt_build::{CxxQtBuilder, QmlModule};

fn main() {
    CxxQtBuilder::new_qml_module(QmlModule::new("io.suntitle.diamant").qml_file("qml/diamant.qml"))
        .file("src/lib.rs")
        .qt_module("Network")
        .qrc_resources(["qml/assets/diamant.svg", "qml/assets/empty-nodes.svg"])
        .build();
}
