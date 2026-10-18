#include "pch.h"
#include "App.h"
#include "App.xaml.g.hpp"
#include "MainWindow.xaml.h"
#include "XamlMetaDataProvider.g.cpp"
#include "XamlTypeInfo.Impl.g.cpp"
#include "XamlTypeInfo.g.cpp"
using namespace winrt;
using namespace Microsoft::UI::Xaml;
namespace winrt::StudioUI::implementation {
App::App() { InitializeComponent(); }
void App::OnLaunched(LaunchActivatedEventArgs const &) {
  window = make<MainWindow>();
  window.Activate();
}
} // namespace winrt::StudioUI::implementation
