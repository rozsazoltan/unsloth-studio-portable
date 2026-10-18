#include "pch.h"
#include "MainWindow.xaml.h"
#include "MainWindow.g.cpp"
#include "MainWindow.xaml.g.hpp"
#include "resource.h"
#include <microsoft.ui.xaml.window.h>
#include <winrt/Microsoft.UI.Windowing.h>
#include <winrt/Microsoft.UI.Xaml.Input.h>
#include <winrt/Windows.System.h>
#include <algorithm>

using namespace winrt;
using namespace Microsoft::UI::Xaml;
using namespace Microsoft::UI::Xaml::Controls;

namespace winrt::StudioUI::implementation {

MainWindow::MainWindow() {
  InitializeComponent();

  HWND hwnd{};
  auto windowNative = try_as<::IWindowNative>();
  if (windowNative && SUCCEEDED(windowNative->get_WindowHandle(&hwnd)) && hwnd) {
    hwnd_ = hwnd;
    const auto instance = GetModuleHandleW(nullptr);
    const auto largeIcon = static_cast<HICON>(LoadImageW(
        instance, MAKEINTRESOURCEW(IDI_UNSLOTH_STUDIO), IMAGE_ICON,
        GetSystemMetrics(SM_CXICON), GetSystemMetrics(SM_CYICON), LR_SHARED));
    const auto smallIcon = static_cast<HICON>(LoadImageW(
        instance, MAKEINTRESOURCEW(IDI_UNSLOTH_STUDIO), IMAGE_ICON,
        GetSystemMetrics(SM_CXSMICON), GetSystemMetrics(SM_CYSMICON),
        LR_SHARED));
    if (largeIcon) {
      SendMessageW(hwnd, WM_SETICON, ICON_BIG,
                   reinterpret_cast<LPARAM>(largeIcon));
    }
    if (smallIcon) {
      SendMessageW(hwnd, WM_SETICON, ICON_SMALL,
                   reinterpret_cast<LPARAM>(smallIcon));
    }
  }

  appWindow_ = AppWindow();
  MONITORINFO monitor{sizeof(monitor)};
  if (hwnd_ && GetMonitorInfoW(MonitorFromWindow(hwnd_, MONITOR_DEFAULTTONEAREST),
                               &monitor)) {
    const auto dpi = GetDpiForWindow(hwnd_);
    const auto width = std::min<int>(MulDiv(1080, dpi, 96),
                                     monitor.rcWork.right - monitor.rcWork.left);
    const auto height = std::min<int>(MulDiv(760, dpi, 96),
                                      monitor.rcWork.bottom - monitor.rcWork.top);
    appWindow_.MoveAndResize(
        {monitor.rcWork.left + (monitor.rcWork.right - monitor.rcWork.left - width) / 2,
         monitor.rcWork.top + (monitor.rcWork.bottom - monitor.rcWork.top - height) / 2,
         width, height});
  }
  ExtendsContentIntoTitleBar(true);
  SetTitleBar(TitleBarDragRegion());
  appWindow_.TitleBar().IconShowOptions(
      Microsoft::UI::Windowing::IconShowOptions::HideIconAndSystemMenu);
  UpdateTitleBarLayout();

  auto weak = get_weak();
  appWindowChangedToken_ = appWindow_.Changed(
      [weak](auto const &, auto const &args) {
        if (args.DidPositionChange() || args.DidSizeChange()) {
          if (auto window = weak.get()) {
            window->UpdateTitleBarLayout();
          }
        }
      });
  navigationLoadedToken_ = Navigation().Loaded(
      [weak](auto const &sender, auto const &) {
        if (auto window = weak.get()) {
          window->xamlRoot_ = sender.as<NavigationView>().XamlRoot();
          window->xamlRootChangedToken_ = window->xamlRoot_.Changed(
              [weak](auto const &, auto const &) {
                if (auto liveWindow = weak.get()) {
                  liveWindow->UpdateTitleBarLayout();
                }
              });
          window->UpdateTitleBarLayout();
        }
      });

  Input::KeyboardAccelerator developerShortcut;
  developerShortcut.Key(Windows::System::VirtualKey::F12);
  developerShortcut.Invoked(
      [weak](Input::KeyboardAccelerator const &,
             Input::KeyboardAcceleratorInvokedEventArgs const &args) {
        if (auto window = weak.get()) {
          window->OpenDeveloperPanel();
          args.Handled(true);
        }
      });
  Navigation().KeyboardAcceleratorPlacementMode(
      Input::KeyboardAcceleratorPlacementMode::Hidden);
  Navigation().KeyboardAccelerators().Append(developerShortcut);

  Navigation().SelectedItem(NavigationGeneralItem());
}

MainWindow::~MainWindow() {
  if (appWindow_ && appWindowChangedToken_) {
    appWindow_.Changed(appWindowChangedToken_);
  }
  if (xamlRoot_ && xamlRootChangedToken_) {
    xamlRoot_.Changed(xamlRootChangedToken_);
  }
  if (navigationLoadedToken_) {
    Navigation().Loaded(navigationLoadedToken_);
  }
}

void MainWindow::UpdateTitleBarLayout() {
  if (!appWindow_) {
    return;
  }
  if (!hwnd_) {
    return;
  }
  const auto dpi = GetDpiForWindow(hwnd_);
  if (!dpi) {
    return;
  }

  const auto scale = static_cast<double>(dpi) / 96.0;
  const auto titleBar = appWindow_.TitleBar();
  const auto nativeHeight = titleBar.Height();
  const auto heightDip = nativeHeight > 0
                             ? static_cast<double>(nativeHeight) / scale
                             : 32.0;
  TitleBarRow().Height(
      GridLengthHelper::FromValueAndType(heightDip, GridUnitType::Pixel));
  TitleBarDragRegion().Height(heightDip);
  PageContent().Margin(Thickness{0, heightDip, 0, 0});
  TitleBarDragRegion().Margin(
      Thickness{static_cast<double>(titleBar.LeftInset()) / scale, 0,
                static_cast<double>(titleBar.RightInset()) / scale, 0});
}

void MainWindow::OnNavigationChanged(
    NavigationView const &,
    NavigationViewSelectionChangedEventArgs const &args) {
  auto item = args.SelectedItem().try_as<NavigationViewItem>();
  if (!item) {
    return;
  }
  const auto tag = unbox_value<hstring>(item.Tag());
  ShowPage(std::wstring(tag.c_str()));
}

void MainWindow::ShowPage(std::wstring const &page) {
  GeneralPage().Visibility(Visibility::Collapsed);
  UpdatePage().Visibility(Visibility::Collapsed);
  ExperimentalPage().Visibility(Visibility::Collapsed);
  AboutPage().Visibility(Visibility::Collapsed);
  DeveloperPage().Visibility(Visibility::Collapsed);

  PageEyebrow().Text(L"PREFERENCES");
  PageSummary().Text(L"");
  if (page == L"Update") {
    PageEyebrow().Text(L"UPDATES");
    PageTitle().Text(L"Update");
    PageSummary().Text(L"Launcher updates are not available in this shell.");
    UpdatePage().Visibility(Visibility::Visible);
  } else if (page == L"Experimental") {
    PageEyebrow().Text(L"EXPERIMENTAL");
    PageTitle().Text(L"Experimental");
    ExperimentalPage().Visibility(Visibility::Visible);
  } else if (page == L"About") {
    PageEyebrow().Text(L"ABOUT");
    PageTitle().Text(L"About");
    PageSummary().Text(L"Application identity, licensing, and notices.");
    AboutPage().Visibility(Visibility::Visible);
  } else if (page == L"Developer") {
    PageEyebrow().Text(L"DEVELOPER");
    PageTitle().Text(L"Developer panel");
    DeveloperPage().Visibility(Visibility::Visible);
  } else {
    PageEyebrow().Text(L"PREFERENCES");
    PageTitle().Text(L"General");
    PageSummary().Text(L"Unsloth Studio Desktop settings.");
    GeneralPage().Visibility(Visibility::Visible);
  }
  PageScroll().ChangeView(nullptr, 0.0, nullptr);
}

void MainWindow::OpenDeveloperPanel() {
  if (developerPanelActive_) {
    return;
  }
  const auto selected = Navigation().SelectedItem().try_as<NavigationViewItem>();
  if (selected && selected != developerNavigationItem_) {
    const auto tag = unbox_value<hstring>(selected.Tag());
    developerReturnPage_ = std::wstring(tag.c_str());
  } else {
    developerReturnPage_ = L"General";
  }
  if (!developerNavigationItem_) {
    developerNavigationItem_ = NavigationViewItem();
    developerNavigationItem_.Content(box_value(L"Developer"));
    developerNavigationItem_.Tag(box_value(L"Developer"));
    developerNavigationItem_.Icon(SymbolIcon(Symbol::Repair));
    Navigation().FooterMenuItems().Append(developerNavigationItem_);
  }
  developerNavigationItem_.Visibility(Visibility::Visible);
  NavigationUpdateItem().Visibility(
      ForceUpdateNavigationToggle().IsOn() ? Visibility::Visible
                                           : Visibility::Collapsed);
  developerPanelActive_ = true;
  Navigation().SelectedItem(developerNavigationItem_);
}

void MainWindow::DeveloperOffClicked(IInspectable const &,
                                     RoutedEventArgs const &) {
  developerPanelActive_ = false;
  if (developerNavigationItem_) {
    developerNavigationItem_.Visibility(Visibility::Collapsed);
  }
  NavigationUpdateItem().Visibility(
      ForceUpdateNavigationToggle().IsOn() ? Visibility::Visible
                                           : Visibility::Collapsed);
  for (auto const &item : Navigation().MenuItems()) {
    auto navItem = item.try_as<NavigationViewItem>();
    if (navItem &&
        std::wstring(unbox_value<hstring>(navItem.Tag()).c_str()) ==
            developerReturnPage_) {
      Navigation().SelectedItem(item);
      return;
    }
  }
  for (auto const &item : Navigation().FooterMenuItems()) {
    auto navItem = item.try_as<NavigationViewItem>();
    if (navItem && navItem != developerNavigationItem_ &&
        std::wstring(unbox_value<hstring>(navItem.Tag()).c_str()) ==
            developerReturnPage_) {
      Navigation().SelectedItem(item);
      return;
    }
  }
  Navigation().SelectedItem(NavigationGeneralItem());
}

void MainWindow::DeveloperResetClicked(IInspectable const &sender,
                                       RoutedEventArgs const &) {
  if (auto toggle = sender.try_as<ToggleSwitch>()) {
    NavigationUpdateItem().Visibility(toggle.IsOn() ? Visibility::Visible
                                                    : Visibility::Collapsed);
    return;
  }
  developerReturnPage_ = L"General";
}

} // namespace winrt::StudioUI::implementation
