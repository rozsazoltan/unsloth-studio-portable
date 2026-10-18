#pragma once

#include "MainWindow.g.h"
#include <winrt/Microsoft.UI.Windowing.h>

namespace winrt::StudioUI::implementation {

struct MainWindow : MainWindowT<MainWindow> {
  MainWindow();
  ~MainWindow();

  void OnNavigationChanged(
      Microsoft::UI::Xaml::Controls::NavigationView const &,
      Microsoft::UI::Xaml::Controls::NavigationViewSelectionChangedEventArgs const &);
  void DeveloperOffClicked(IInspectable const &,
                           Microsoft::UI::Xaml::RoutedEventArgs const &);
  void DeveloperResetClicked(IInspectable const &,
                             Microsoft::UI::Xaml::RoutedEventArgs const &);

private:
  void UpdateTitleBarLayout();
  void ShowPage(std::wstring const &page);
  void OpenDeveloperPanel();

  Microsoft::UI::Windowing::AppWindow appWindow_{nullptr};
  Microsoft::UI::Xaml::XamlRoot xamlRoot_{nullptr};
  Microsoft::UI::Xaml::Controls::NavigationViewItem developerNavigationItem_{nullptr};
  winrt::event_token appWindowChangedToken_{};
  winrt::event_token navigationLoadedToken_{};
  winrt::event_token xamlRootChangedToken_{};
  std::wstring developerReturnPage_{L"General"};
  HWND hwnd_{};
  bool developerPanelActive_{false};
};

} // namespace winrt::StudioUI::implementation

namespace winrt::StudioUI::factory_implementation {
struct MainWindow : MainWindowT<MainWindow, implementation::MainWindow> {};
} // namespace winrt::StudioUI::factory_implementation
