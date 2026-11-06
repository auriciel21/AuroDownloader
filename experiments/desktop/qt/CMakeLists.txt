cmake_minimum_required(VERSION 3.21)
project(auro_qt VERSION 0.1 LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

find_package(Qt6 6.8 REQUIRED COMPONENTS Quick QuickControls2 Network Widgets LinguistTools)
qt_standard_project_setup(REQUIRES 6.8 I18N_SOURCE_LANGUAGE fr I18N_TRANSLATED_LANGUAGES en)

qt_add_executable(auro_qt WIN32
    main.cpp
    engineclient.h engineclient.cpp
    taskmodel.h taskmodel.cpp
)

qt_add_qml_module(auro_qt
    URI Auro
    VERSION 1.0
    QML_FILES Main.qml
)

qt_add_translations(auro_qt
    TS_FILE_BASE auro
    TS_FILE_DIR i18n
    RESOURCE_PREFIX /i18n
)

target_link_libraries(auro_qt PRIVATE
    Qt6::Quick Qt6::QuickControls2 Qt6::Network Qt6::Widgets)
