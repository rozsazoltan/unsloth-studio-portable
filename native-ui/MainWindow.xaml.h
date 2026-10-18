#pragma once

#include "MainWindow.g.h"

namespace winrt::StudioUI::implementation {

struct MainWindow : MainWindowT<MainWindow> {
  MainWindow();
};

} // namespace winrt::StudioUI::implementation

namespace winrt::StudioUI::factory_implementation {
struct MainWindow : MainWindowT<MainWindow, implementation::MainWindow> {};
} // namespace winrt::StudioUI::factory_implementation
