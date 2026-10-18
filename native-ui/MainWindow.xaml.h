#pragma once

#include "Bridge.h"
#include "MainWindow.g.h"

namespace winrt::StudioUI::implementation {

struct MainWindow : MainWindowT<MainWindow> {
  MainWindow();
  ~MainWindow();

  void NavChanged(Microsoft::UI::Xaml::Controls::NavigationView const &,
                  Microsoft::UI::Xaml::Controls::
                      NavigationViewSelectionChangedEventArgs const &);
  void InstallClicked(IInspectable const &,
                      Microsoft::UI::Xaml::RoutedEventArgs const &);
  void UpdateClicked(IInspectable const &,
                     Microsoft::UI::Xaml::RoutedEventArgs const &);
  void StartClicked(IInspectable const &,
                    Microsoft::UI::Xaml::RoutedEventArgs const &);
  void StopClicked(IInspectable const &,
                   Microsoft::UI::Xaml::RoutedEventArgs const &);
  void OpenClicked(IInspectable const &,
                   Microsoft::UI::Xaml::RoutedEventArgs const &);
  void CheckClicked(IInspectable const &,
                    Microsoft::UI::Xaml::RoutedEventArgs const &);
  void RemoveClicked(IInspectable const &,
                     Microsoft::UI::Xaml::RoutedEventArgs const &);
  void BrowseClicked(IInspectable const &,
                     Microsoft::UI::Xaml::RoutedEventArgs const &);
  void SaveClicked(IInspectable const &,
                   Microsoft::UI::Xaml::RoutedEventArgs const &);
  void
  ParentChanged(IInspectable const &,
                Microsoft::UI::Xaml::Controls::TextChangedEventArgs const &);

private:
  Bridge m_bridge;
  Microsoft::UI::Dispatching::DispatcherQueue m_dispatcher{nullptr};
  Microsoft::UI::Dispatching::DispatcherQueueTimer m_pollTimer{nullptr};
  winrt::Windows::Data::Json::JsonObject m_state{nullptr};
  std::wstring m_lastRoot;
  std::wstring m_lastDefaultHf;
  std::wstring m_previousRemovalToken;
  std::wstring m_removalScope;
  uint64_t m_removalRequestId{0};
  uint64_t m_saveRequestId{0};
  uint64_t m_removalBaselineRevision{0};
  bool m_snapshotOutstanding{false};
  bool m_settingsInitialized{false};
  bool m_applySettingsAfterSave{false};
  bool m_removalPrepareAcknowledged{false};
  bool m_removalDialogOpen{false};
  bool m_consentDialogOpen{false};
  bool m_connected{false};

  void Send(std::wstring const &command);
  void Receive(winrt::Windows::Data::Json::JsonObject const &message);
  void UpdateView();
  void ApplySettings(winrt::Windows::Data::Json::JsonObject const &settings);
  void ShowError(std::wstring const &message);
  void AppendLog(std::wstring const &message);
  void RequestSnapshot();
  winrt::Windows::Foundation::IAsyncAction
  ShowSetupConsentAsync(std::wstring command);
  winrt::Windows::Foundation::IAsyncAction RemoveAsync();
  winrt::Windows::Foundation::IAsyncAction
  ShowRemovalConfirmationAsync(winrt::Windows::Data::Json::JsonObject removal);
  void MaybeShowRemovalConfirmation();
};

} // namespace winrt::StudioUI::implementation

namespace winrt::StudioUI::factory_implementation {
struct MainWindow : MainWindowT<MainWindow, implementation::MainWindow> {};
} // namespace winrt::StudioUI::factory_implementation
