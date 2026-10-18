#include "pch.h"
#include "MainWindow.xaml.h"
#include "MainWindow.g.cpp"
#include "MainWindow.xaml.g.hpp"

#include <algorithm>
#include <cwctype>
#include <shlobj.h>
#include <sstream>

using namespace winrt;
using namespace Microsoft::UI::Xaml;
using namespace Microsoft::UI::Xaml::Controls;
using namespace Windows::Data::Json;

namespace winrt::StudioUI::implementation {
namespace {

constexpr wchar_t kProductFolder[] = L"Unsloth Studio Desktop";
constexpr size_t kMaximumLogCharacters = 6000;

hstring StringValue(JsonObject const &object, wchar_t const *key,
                    wchar_t const *fallback) {
  if (!object || !object.HasKey(key)) {
    return fallback;
  }
  auto value = object.GetNamedValue(key);
  if (value.ValueType() != JsonValueType::String) {
    return fallback;
  }
  return value.GetString();
}

bool BooleanValue(JsonObject const &object, wchar_t const *key, bool fallback) {
  if (!object || !object.HasKey(key)) {
    return fallback;
  }
  auto value = object.GetNamedValue(key);
  return value.ValueType() == JsonValueType::Boolean ? value.GetBoolean()
                                                     : fallback;
}

double NumberValue(JsonObject const &object, wchar_t const *key,
                   double fallback) {
  if (!object || !object.HasKey(key)) {
    return fallback;
  }
  auto value = object.GetNamedValue(key);
  return value.ValueType() == JsonValueType::Number ? value.GetNumber()
                                                    : fallback;
}

uint64_t RevisionValue(JsonObject const &object) {
  return static_cast<uint64_t>(
      std::max(0.0, NumberValue(object, L"revision", 0)));
}

std::wstring DefaultHfPath(std::wstring const &root) {
  return root.empty() ? std::wstring{} : root + L"\\huggingface";
}

std::wstring ParentFromRoot(std::wstring root) {
  const std::wstring suffix = std::wstring(L"\\") + kProductFolder;
  if (root.size() >= suffix.size() &&
      _wcsicmp(root.c_str() + root.size() - suffix.size(), suffix.c_str()) ==
          0) {
    root.resize(root.size() - suffix.size());
  }
  return root;
}

std::wstring PathsText(JsonArray const &paths, wchar_t const *emptyText) {
  if (!paths || paths.Size() == 0) {
    return emptyText;
  }
  std::wstring result;
  for (uint32_t index = 0; index < paths.Size(); ++index) {
    auto item = paths.GetAt(index);
    if (item.ValueType() != JsonValueType::String) {
      continue;
    }
    if (!result.empty()) {
      result += L"\n";
    }
    result += L"• ";
    result += item.GetString().c_str();
  }
  return result.empty() ? emptyText : result;
}

bool IsStringArray(JsonArray const &paths, bool requireEntry) {
  if (!paths || (requireEntry && paths.Size() == 0)) {
    return false;
  }
  for (uint32_t index = 0; index < paths.Size(); ++index) {
    if (paths.GetAt(index).ValueType() != JsonValueType::String) {
      return false;
    }
  }
  return true;
}

} // namespace

MainWindow::MainWindow() {
  InitializeComponent();
  m_dispatcher =
      Microsoft::UI::Dispatching::DispatcherQueue::GetForCurrentThread();

  auto weakWindow = get_weak();
  m_bridge.Start([weakWindow](JsonObject const &message) {
    if (auto window = weakWindow.get()) {
      auto dispatcher = window->m_dispatcher;
      if (dispatcher) {
        dispatcher.TryEnqueue([weakWindow, message] {
          if (auto liveWindow = weakWindow.get()) {
            liveWindow->Receive(message);
          }
        });
      }
    }
  });

  m_pollTimer = m_dispatcher.CreateTimer();
  m_pollTimer.Interval(std::chrono::seconds(1));
  m_pollTimer.IsRepeating(true);
  m_pollTimer.Tick([weakWindow](auto const &, auto const &) {
    if (auto window = weakWindow.get()) {
      if (!window->m_connected) {
        return;
      }
      window->RequestSnapshot();
    }
  });

  if (m_bridge.IsConfigured()) {
    Connection().Text(L"Connecting to launcher…");
  } else {
    Connection().Text(L"Launcher pipe not supplied");
    ShowError(L"Start this companion through StudioUI.exe --pipe <name>. No "
              L"local actions are simulated.");
  }
}

MainWindow::~MainWindow() {
  if (m_pollTimer) {
    m_pollTimer.Stop();
  }
  m_bridge.Stop();
}

void MainWindow::Send(std::wstring const &command) { m_bridge.Send(command); }

void MainWindow::Receive(JsonObject const &message) {
  try {
    auto type = StringValue(message, L"type", L"");
    if (type == L"bridge_status") {
      m_connected = BooleanValue(message, L"connected", false);
      auto text =
          StringValue(message, L"text", L"Launcher connection changed.");
      Connection().Text(text);
      AppendLog(text.c_str());
      if (m_connected) {
        Banner().IsOpen(false);
        RequestSnapshot();
      } else {
        ShowError(text.c_str());
      }
      UpdateView();
      return;
    }

    if (type == L"snapshot") {
      auto stateValue = message.GetNamedValue(L"state");
      if (stateValue.ValueType() != JsonValueType::Object) {
        m_snapshotOutstanding = false;
        ShowError(L"Launcher sent a snapshot without a valid state object.");
        return;
      }
      m_state = stateValue.GetObject();
      m_snapshotOutstanding = false;
      UpdateView();

      if (m_settingsInitialized && m_applySettingsAfterSave) {
        auto settingsValue = m_state.GetNamedValue(L"settings", nullptr);
        if (settingsValue &&
            settingsValue.ValueType() == JsonValueType::Object) {
          ApplySettings(settingsValue.GetObject());
        }
        m_applySettingsAfterSave = false;
      } else if (!m_settingsInitialized) {
        auto settingsValue = m_state.GetNamedValue(L"settings", nullptr);
        if (settingsValue &&
            settingsValue.ValueType() == JsonValueType::Object) {
          ApplySettings(settingsValue.GetObject());
          m_settingsInitialized = true;
        }
      }
      MaybeShowRemovalConfirmation();
      return;
    }

    if (type == L"log") {
      AppendLog(StringValue(message, L"text", L"Launcher activity.").c_str());
      return;
    }

    if (type == L"reply") {
      const auto requestId =
          static_cast<uint64_t>(NumberValue(message, L"id", 0));
      const bool succeeded = BooleanValue(message, L"ok", false);
      if (requestId == m_removalRequestId && m_removalRequestId != 0) {
        m_removalPrepareAcknowledged = succeeded;
        if (!succeeded) {
          m_removalRequestId = 0;
          ShowError(
              L"Launcher rejected removal preparation. No data was removed.");
        }
        MaybeShowRemovalConfirmation();
      }
      if (requestId == m_saveRequestId && m_saveRequestId != 0) {
        m_saveRequestId = 0;
        if (succeeded) {
          m_applySettingsAfterSave = true;
          RequestSnapshot();
          AppendLog(L"Settings saved.");
        } else {
          ShowError(
              L"Launcher rejected settings. Review the paths and try again.");
        }
      }
      if (!succeeded) {
        auto errorValue = message.GetNamedValue(L"error", nullptr);
        if (errorValue && errorValue.ValueType() == JsonValueType::Object) {
          auto error = errorValue.GetObject();
          ShowError(
              StringValue(error, L"message", L"Launcher rejected the request.")
                  .c_str());
        }
      }
    }
  } catch (hresult_error const &) {
    ShowError(
        L"Launcher sent malformed data. The companion ignored that message.");
    AppendLog(L"Malformed launcher message ignored.");
  } catch (...) {
    ShowError(L"Could not process launcher message.");
  }
}

void MainWindow::UpdateView() {
  if (!m_state) {
    return;
  }

  const auto phase = StringValue(m_state, L"phase", L"Unknown");
  const auto operationValue = m_state.GetNamedValue(L"operation", nullptr);
  const bool operationActive =
      operationValue && operationValue.ValueType() == JsonValueType::Object;
  const bool running =
      phase == L"Running" || phase == L"Starting" || phase == L"Stopping";
  const bool modifying = operationActive || phase == L"Installing" ||
                         phase == L"Updating" || phase == L"Removing" ||
                         phase == L"Starting" || phase == L"Stopping";

  Phase().Text(phase);
  InstallStatus().Text(phase == L"Ready" || phase == L"Running"
                           ? L"Installed and ready"
                           : phase);
  if (operationActive) {
    auto operation = operationValue.GetObject();
    InstallStatus().Text(StringValue(operation, L"detail", phase.c_str()));
  }

  const auto installedCommit =
      StringValue(m_state, L"installed_commit", L"not available");
  const auto upstreamCommit =
      StringValue(m_state, L"upstream_commit", L"not checked");
  Commit().Text(L"Installed: " + installedCommit + L"  ·  Latest: " +
                upstreamCommit);

  const auto updateStatus = StringValue(m_state, L"update_status", L"unknown");
  if (updateStatus == L"available") {
    Banner().Severity(InfoBarSeverity::Warning);
    Banner().Message(L"An Unsloth Studio update is available.");
    Banner().IsOpen(true);
  } else if (updateStatus == L"error") {
    Banner().Severity(InfoBarSeverity::Error);
    Banner().Message(L"Update check failed. See launcher activity.");
    Banner().IsOpen(true);
  } else if (m_connected) {
    Banner().IsOpen(false);
  }

  const bool installable = phase == L"Absent" || phase == L"Failed";
  const bool removable = installable || phase == L"Ready";
  InstallButton().IsEnabled(m_connected && !modifying && installable);
  UpdateButton().IsEnabled(m_connected && !modifying && phase == L"Ready");
  StartButton().IsEnabled(m_connected && !modifying && phase == L"Ready");
  StopButton().IsEnabled(m_connected &&
                         (phase == L"Running" || phase == L"Starting"));
  OpenButton().IsEnabled(m_connected && phase == L"Running");
  CheckButton().IsEnabled(m_connected && !modifying);
  RemoveButton().IsEnabled(m_connected && !modifying && !running && removable);
  SaveButton().IsEnabled(m_connected && !modifying);

  auto errorValue = m_state.GetNamedValue(L"last_error", nullptr);
  if (errorValue && errorValue.ValueType() == JsonValueType::Object) {
    auto error = errorValue.GetObject();
    auto message = StringValue(error, L"message", L"");
    if (!message.empty()) {
      Banner().Severity(InfoBarSeverity::Error);
      Banner().Message(message);
      Banner().IsOpen(true);
    }
  }
}

void MainWindow::ApplySettings(JsonObject const &settings) {
  auto root = std::wstring(StringValue(settings, L"root", L"").c_str());
  auto newDefaultHf = DefaultHfPath(root);
  ParentPath().Text(ParentFromRoot(root));
  HfPath().Text(StringValue(settings, L"hf_home", newDefaultHf.c_str()));
  Tray().IsOn(BooleanValue(settings, L"tray_enabled", true));
  Startup().IsOn(BooleanValue(settings, L"startup_enabled", false));
  DesktopShortcut().IsOn(BooleanValue(settings, L"desktop_shortcut", false));
  StartMenuShortcut().IsOn(
      BooleanValue(settings, L"start_menu_shortcut", false));
  m_lastRoot = root;
  m_lastDefaultHf = std::move(newDefaultHf);
}

void MainWindow::ShowError(std::wstring const &message) {
  Banner().Severity(InfoBarSeverity::Error);
  Banner().Message(message);
  Banner().IsOpen(true);
}

void MainWindow::AppendLog(std::wstring const &message) {
  if (message.empty()) {
    return;
  }
  auto current = std::wstring(Log().Text().c_str());
  if (current == L"No launcher messages yet.") {
    current.clear();
  }
  if (!current.empty()) {
    current += L"\n";
  }
  auto safeMessage = message.substr(0, 1200);
  auto lowercase = safeMessage;
  std::transform(lowercase.begin(), lowercase.end(), lowercase.begin(),
                 [](wchar_t character) {
                   return static_cast<wchar_t>(std::towlower(character));
                 });
  if (lowercase.find(L"token") != std::wstring::npos ||
      lowercase.find(L"password") != std::wstring::npos ||
      lowercase.find(L"authorization") != std::wstring::npos) {
    safeMessage = L"Sensitive launcher activity omitted.";
  }
  current += safeMessage;
  if (current.size() > kMaximumLogCharacters) {
    current.erase(0, current.size() - kMaximumLogCharacters);
    auto firstNewline = current.find(L'\n');
    if (firstNewline != std::wstring::npos) {
      current.erase(0, firstNewline + 1);
    }
  }
  Log().Text(current);
}

void MainWindow::RequestSnapshot() {
  if (!m_connected || m_snapshotOutstanding) {
    return;
  }
  m_snapshotOutstanding = true;
  Send(L"get_snapshot");
}

void MainWindow::NavChanged(
    Controls::NavigationView const &,
    Controls::NavigationViewSelectionChangedEventArgs const &args) {
  auto item = args.SelectedItem().try_as<Controls::NavigationViewItem>();
  if (!item) {
    return;
  }
  auto tag = unbox_value<hstring>(item.Tag());
  Overview().Visibility(tag == L"overview" ? Visibility::Visible
                                           : Visibility::Collapsed);
  Settings().Visibility(tag == L"settings" ? Visibility::Visible
                                           : Visibility::Collapsed);
  About().Visibility(tag == L"about" ? Visibility::Visible
                                     : Visibility::Collapsed);
  Experimental().Visibility(tag == L"experimental" ? Visibility::Visible
                                                   : Visibility::Collapsed);
}

void MainWindow::InstallClicked(IInspectable const &, RoutedEventArgs const &) {
  ShowSetupConsentAsync(L"install");
}

void MainWindow::UpdateClicked(IInspectable const &, RoutedEventArgs const &) {
  ShowSetupConsentAsync(L"update");
}

Windows::Foundation::IAsyncAction
MainWindow::ShowSetupConsentAsync(std::wstring command) {
  auto lifetime = get_strong();
  if (m_consentDialogOpen) {
    co_return;
  }
  m_consentDialogOpen = true;
  const bool update = command == L"update";
  ContentDialog dialog;
  dialog.XamlRoot(Content().XamlRoot());
  dialog.Title(box_value(update ? L"Approve upstream update setup"
                                : L"Approve upstream setup"));
  dialog.Content(
      box_value(L"Setup may install the Visual C++ runtime or CMake through "
                L"winget, change your user PATH, and affect external WebView "
                L"cache data. Installer scripts run with your Windows account "
                L"permissions and are not fully isolated. Approve this "
                L"operation only if you accept these changes."));
  dialog.PrimaryButtonText(update ? L"Approve update" : L"Approve install");
  dialog.CloseButtonText(L"Cancel");
  dialog.DefaultButton(ContentDialogButton::Close);

  ContentDialogResult result{};
  try {
    result = co_await dialog.ShowAsync();
  } catch (...) {
    m_consentDialogOpen = false;
    ShowError(L"Could not open the upstream setup approval dialog.");
    co_return;
  }
  m_consentDialogOpen = false;
  if (result != ContentDialogResult::Primary) {
    co_return;
  }
  JsonObject fields;
  fields.Insert(L"upstream_setup_approved",
                JsonValue::CreateBooleanValue(true));
  m_bridge.Send(command, fields);
  AppendLog(update ? L"Upstream setup approved for this update only."
                   : L"Upstream setup approved for this install only.");
}

void MainWindow::StartClicked(IInspectable const &, RoutedEventArgs const &) {
  Send(L"start");
}
void MainWindow::StopClicked(IInspectable const &, RoutedEventArgs const &) {
  Send(L"stop");
}
void MainWindow::OpenClicked(IInspectable const &, RoutedEventArgs const &) {
  Send(L"open_studio");
}
void MainWindow::CheckClicked(IInspectable const &, RoutedEventArgs const &) {
  Send(L"check_updates");
}

void MainWindow::BrowseClicked(IInspectable const &, RoutedEventArgs const &) {
  BROWSEINFOW browse{};
  browse.ulFlags = BIF_RETURNONLYFSDIRS | BIF_NEWDIALOGSTYLE;
  auto selected = SHBrowseForFolderW(&browse);
  if (!selected) {
    return;
  }

  wchar_t path[MAX_PATH]{};
  if (SHGetPathFromIDListW(selected, path)) {
    const auto priorDefault = m_lastDefaultHf;
    ParentPath().Text(path);
    if (HfPath().Text().empty() || HfPath().Text() == priorDefault) {
      HfPath().Text(DefaultHfPath(std::wstring(path) + L"\\" + kProductFolder));
    }
  }
  CoTaskMemFree(selected);
}

void MainWindow::ParentChanged(IInspectable const &,
                               Controls::TextChangedEventArgs const &) {
  const auto currentHf = std::wstring(HfPath().Text().c_str());
  const auto parent = std::wstring(ParentPath().Text().c_str());
  const auto productRoot =
      parent.empty() ? std::wstring{} : parent + L"\\" + kProductFolder;
  const auto newDefaultHf = DefaultHfPath(productRoot);
  if (currentHf.empty() || currentHf == m_lastDefaultHf) {
    HfPath().Text(newDefaultHf);
  }
  m_lastDefaultHf = newDefaultHf;
}

void MainWindow::SaveClicked(IInspectable const &, RoutedEventArgs const &) {
  if (!m_state || m_saveRequestId != 0) {
    ShowError(
        L"Launcher settings are not available or a save is already pending.");
    return;
  }

  JsonObject settings;
  std::wstring root = ParentPath().Text().c_str();
  while (!root.empty() && (root.back() == L'\\' || root.back() == L'/')) {
    root.pop_back();
  }
  root += L"\\";
  root += kProductFolder;
  settings.Insert(L"root", JsonValue::CreateStringValue(root));
  settings.Insert(L"hf_home", JsonValue::CreateStringValue(HfPath().Text()));
  settings.Insert(L"tray_enabled",
                  JsonValue::CreateBooleanValue(Tray().IsOn()));
  settings.Insert(L"startup_enabled",
                  JsonValue::CreateBooleanValue(Startup().IsOn()));
  settings.Insert(L"desktop_shortcut",
                  JsonValue::CreateBooleanValue(DesktopShortcut().IsOn()));
  settings.Insert(L"start_menu_shortcut",
                  JsonValue::CreateBooleanValue(StartMenuShortcut().IsOn()));
  auto priorSettingsValue = m_state.GetNamedValue(L"settings", nullptr);
  const double port =
      priorSettingsValue &&
              priorSettingsValue.ValueType() == JsonValueType::Object
          ? NumberValue(priorSettingsValue.GetObject(), L"port", 8888)
          : 8888;
  settings.Insert(L"port", JsonValue::CreateNumberValue(port));

  JsonObject fields;
  fields.Insert(L"settings", settings);
  m_saveRequestId = m_bridge.Send(L"save_settings", fields);
}

void MainWindow::RemoveClicked(IInspectable const &, RoutedEventArgs const &) {
  RemoveAsync();
}

Windows::Foundation::IAsyncAction MainWindow::RemoveAsync() {
  if (!m_state || m_removalRequestId != 0 || m_removalDialogOpen) {
    co_return;
  }
  const auto operationValue = m_state.GetNamedValue(L"operation", nullptr);
  if (operationValue && operationValue.ValueType() == JsonValueType::Object) {
    ShowError(
        L"Wait for the current operation to finish before preparing removal.");
    co_return;
  }

  auto lifetime = get_strong();
  m_removalDialogOpen = true;
  ContentDialog dialog;
  dialog.XamlRoot(Content().XamlRoot());
  dialog.Title(box_value(L"Choose removal scope"));
  dialog.Content(box_value(
      L"External cache paths are preserved. All-owned-data removal may delete "
      L"the Hugging Face folder inside the product root. The launcher will "
      L"show exact deletion and preserved paths before confirmation."));
  dialog.PrimaryButtonText(L"Remove runtime");
  dialog.SecondaryButtonText(L"Remove all owned data");
  dialog.CloseButtonText(L"Cancel");
  dialog.DefaultButton(ContentDialogButton::Close);

  ContentDialogResult result{};
  try {
    result = co_await dialog.ShowAsync();
  } catch (...) {
    m_removalDialogOpen = false;
    ShowError(L"Could not open removal scope dialog.");
    co_return;
  }
  if (result == ContentDialogResult::None) {
    m_removalDialogOpen = false;
    co_return;
  }
  const auto currentPhase = StringValue(m_state, L"phase", L"Unknown");
  const auto currentOperation = m_state.GetNamedValue(L"operation", nullptr);
  const bool operationActive =
      currentOperation && currentOperation.ValueType() == JsonValueType::Object;
  if (operationActive ||
      (currentPhase != L"Absent" && currentPhase != L"Failed" &&
       currentPhase != L"Ready")) {
    m_removalDialogOpen = false;
    ShowError(L"Launcher state changed. Refresh and retry removal when Studio "
              L"is stopped and no operation is active.");
    co_return;
  }

  auto removalValue = m_state.GetNamedValue(L"removal", nullptr);
  m_previousRemovalToken.clear();
  if (removalValue && removalValue.ValueType() == JsonValueType::Object) {
    m_previousRemovalToken =
        StringValue(removalValue.GetObject(), L"token", L"").c_str();
  }
  m_removalBaselineRevision = RevisionValue(m_state);
  m_removalScope =
      result == ContentDialogResult::Secondary ? L"all_owned_data" : L"runtime";
  m_removalPrepareAcknowledged = false;
  m_removalDialogOpen = false;
  JsonObject fields;
  fields.Insert(L"scope", JsonValue::CreateStringValue(m_removalScope));
  m_removalRequestId = m_bridge.Send(L"prepare_removal", fields);
}

void MainWindow::MaybeShowRemovalConfirmation() {
  if (!m_removalPrepareAcknowledged || m_removalDialogOpen || !m_state) {
    return;
  }
  auto removalValue = m_state.GetNamedValue(L"removal", nullptr);
  if (!removalValue || removalValue.ValueType() != JsonValueType::Object ||
      RevisionValue(m_state) <= m_removalBaselineRevision) {
    return;
  }
  auto removal = removalValue.GetObject();
  auto token = StringValue(removal, L"token", L"");
  if (token.empty() || token == m_previousRemovalToken) {
    return;
  }
  auto deletionValue = removal.GetNamedValue(L"paths", nullptr);
  auto preservedValue = removal.GetNamedValue(L"preserved_paths", nullptr);
  if (!deletionValue || deletionValue.ValueType() != JsonValueType::Array ||
      !preservedValue || preservedValue.ValueType() != JsonValueType::Array ||
      !IsStringArray(deletionValue.GetArray(), true) ||
      !IsStringArray(preservedValue.GetArray(), false)) {
    m_removalPrepareAcknowledged = false;
    m_removalRequestId = 0;
    ShowError(L"Launcher did not provide complete removal paths. Nothing was "
              L"removed.");
    return;
  }

  m_removalDialogOpen = true;
  ShowRemovalConfirmationAsync(removal);
}

Windows::Foundation::IAsyncAction
MainWindow::ShowRemovalConfirmationAsync(JsonObject removal) {
  auto lifetime = get_strong();
  const auto token = std::wstring(StringValue(removal, L"token", L"").c_str());
  const auto warning =
      StringValue(removal, L"warning", L"Review each path before confirming.");
  const auto deletePaths = PathsText(removal.GetNamedArray(L"paths", nullptr),
                                     L"No deletion paths reported.");
  const auto preservedPaths =
      PathsText(removal.GetNamedArray(L"preserved_paths", nullptr),
                L"No external paths require preservation.");

  StackPanel content;
  content.Spacing(8);
  TextBlock warningText;
  warningText.Text(warning);
  warningText.TextWrapping(TextWrapping::Wrap);
  content.Children().Append(warningText);
  TextBlock deleteHeading;
  deleteHeading.Text(L"Paths to delete");
  content.Children().Append(deleteHeading);
  TextBlock deleteText;
  deleteText.Text(deletePaths);
  deleteText.TextWrapping(TextWrapping::Wrap);
  content.Children().Append(deleteText);
  TextBlock preservedHeading;
  preservedHeading.Text(L"Preserved paths");
  content.Children().Append(preservedHeading);
  TextBlock preservedText;
  preservedText.Text(preservedPaths);
  preservedText.TextWrapping(TextWrapping::Wrap);
  content.Children().Append(preservedText);
  TextBlock tokenPrompt;
  tokenPrompt.Text(L"Type the exact token below to confirm deletion:");
  tokenPrompt.TextWrapping(TextWrapping::Wrap);
  content.Children().Append(tokenPrompt);
  TextBlock tokenText;
  tokenText.Text(L"Token: " + token);
  tokenText.TextWrapping(TextWrapping::Wrap);
  content.Children().Append(tokenText);
  TextBox tokenEntry;
  tokenEntry.PlaceholderText(L"Confirmation token");
  tokenEntry.AcceptsReturn(false);
  content.Children().Append(tokenEntry);

  ScrollViewer pathScroller;
  pathScroller.MaxHeight(460);
  pathScroller.Content(content);

  ContentDialog dialog;
  dialog.XamlRoot(Content().XamlRoot());
  dialog.Title(box_value(L"Confirm permanent removal"));
  dialog.Content(pathScroller);
  dialog.PrimaryButtonText(L"Delete listed paths");
  dialog.CloseButtonText(L"Cancel");
  dialog.DefaultButton(ContentDialogButton::Close);

  ContentDialogResult result{};
  try {
    result = co_await dialog.ShowAsync();
  } catch (...) {
    m_removalDialogOpen = false;
    m_removalRequestId = 0;
    m_removalPrepareAcknowledged = false;
    ShowError(L"Could not open removal confirmation. Nothing was removed.");
    co_return;
  }
  m_removalDialogOpen = false;
  m_removalRequestId = 0;
  m_removalPrepareAcknowledged = false;
  m_previousRemovalToken = token;
  if (result != ContentDialogResult::Primary || tokenEntry.Text() != token) {
    if (result == ContentDialogResult::Primary) {
      ShowError(L"Confirmation token did not match. Nothing was deleted.");
    }
    co_return;
  }

  JsonObject fields;
  fields.Insert(L"token", JsonValue::CreateStringValue(token));
  m_bridge.Send(L"confirm_removal", fields);
}

} // namespace winrt::StudioUI::implementation
