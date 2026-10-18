#include "pch.h"
#include "MainWindow.xaml.h"
#include "MainWindow.g.cpp"
#include "MainWindow.xaml.g.hpp"

namespace winrt::StudioUI::implementation {

MainWindow::MainWindow() { InitializeComponent(); }

void MainWindow::ShowPage(
    Microsoft::UI::Xaml::Controls::StackPanel const &page) {
  GeneralPage().Visibility(Microsoft::UI::Xaml::Visibility::Collapsed);
  UpdatePage().Visibility(Microsoft::UI::Xaml::Visibility::Collapsed);
  ExperimentalPage().Visibility(Microsoft::UI::Xaml::Visibility::Collapsed);
  AboutPage().Visibility(Microsoft::UI::Xaml::Visibility::Collapsed);
  page.Visibility(Microsoft::UI::Xaml::Visibility::Visible);
}

void MainWindow::GeneralClicked(
    IInspectable const &, Microsoft::UI::Xaml::RoutedEventArgs const &) {
  ShowPage(GeneralPage());
}

void MainWindow::UpdateClicked(
    IInspectable const &, Microsoft::UI::Xaml::RoutedEventArgs const &) {
  ShowPage(UpdatePage());
}

void MainWindow::ExperimentalClicked(
    IInspectable const &, Microsoft::UI::Xaml::RoutedEventArgs const &) {
  ShowPage(ExperimentalPage());
}

void MainWindow::AboutClicked(IInspectable const &,
                              Microsoft::UI::Xaml::RoutedEventArgs const &) {
  ShowPage(AboutPage());
}

} // namespace winrt::StudioUI::implementation
