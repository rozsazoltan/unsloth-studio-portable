#pragma once

#include "MainWindow.g.h"

namespace winrt::StudioUI::implementation {

struct MainWindow : MainWindowT<MainWindow> {
  MainWindow();
  void GeneralClicked(IInspectable const &, Microsoft::UI::Xaml::RoutedEventArgs const &);
  void UpdateClicked(IInspectable const &, Microsoft::UI::Xaml::RoutedEventArgs const &);
  void ExperimentalClicked(IInspectable const &, Microsoft::UI::Xaml::RoutedEventArgs const &);
  void AboutClicked(IInspectable const &, Microsoft::UI::Xaml::RoutedEventArgs const &);

private:
  void ShowPage(Microsoft::UI::Xaml::Controls::StackPanel const &page);
};

} // namespace winrt::StudioUI::implementation

namespace winrt::StudioUI::factory_implementation {
struct MainWindow : MainWindowT<MainWindow, implementation::MainWindow> {};
} // namespace winrt::StudioUI::factory_implementation
