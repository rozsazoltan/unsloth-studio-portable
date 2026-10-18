#include "pch.h"
#include "Bridge.h"

#include <algorithm>
#include <array>
#include <shellapi.h>
#include <tlhelp32.h>

using namespace winrt;
using namespace Windows::Data::Json;

namespace winrt::StudioUI::implementation {
namespace {

constexpr uint32_t kMaximumPayload = 1024 * 1024;
constexpr wchar_t kLocalPipePrefix[] = L"\\\\.\\pipe\\";

struct ApartmentScope {
  ApartmentScope() {
    winrt::init_apartment(winrt::apartment_type::multi_threaded);
  }
  ~ApartmentScope() { winrt::uninit_apartment(); }
};

uint32_t DecodeLength(std::array<unsigned char, 4> const &bytes) {
  return static_cast<uint32_t>(bytes[0]) |
         (static_cast<uint32_t>(bytes[1]) << 8) |
         (static_cast<uint32_t>(bytes[2]) << 16) |
         (static_cast<uint32_t>(bytes[3]) << 24);
}

std::array<unsigned char, 4> EncodeLength(uint32_t length) {
  return {static_cast<unsigned char>(length & 0xff),
          static_cast<unsigned char>((length >> 8) & 0xff),
          static_cast<unsigned char>((length >> 16) & 0xff),
          static_cast<unsigned char>((length >> 24) & 0xff)};
}

DWORD ParentProcessId() {
  auto snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
  if (snapshot == INVALID_HANDLE_VALUE) {
    return 0;
  }

  PROCESSENTRY32W entry{};
  entry.dwSize = sizeof(entry);
  const DWORD currentProcess = GetCurrentProcessId();
  DWORD parentProcess = 0;
  if (Process32FirstW(snapshot, &entry)) {
    do {
      if (entry.th32ProcessID == currentProcess) {
        parentProcess = entry.th32ParentProcessID;
        break;
      }
    } while (Process32NextW(snapshot, &entry));
  }
  CloseHandle(snapshot);
  return parentProcess;
}

bool IsSupportedLocalPipe(std::wstring const &name) {
  return name.size() > std::size(kLocalPipePrefix) - 1 &&
         _wcsnicmp(name.c_str(), kLocalPipePrefix,
                   std::size(kLocalPipePrefix) - 1) == 0;
}

} // namespace

Bridge::Bridge() {
  int argumentCount = 0;
  auto arguments = CommandLineToArgvW(GetCommandLineW(), &argumentCount);
  if (!arguments) {
    return;
  }

  for (int index = 1; index + 1 < argumentCount; ++index) {
    if (std::wstring(arguments[index]) == L"--pipe") {
      m_pipe = arguments[index + 1];
      break;
    }
  }
  LocalFree(arguments);
}

Bridge::~Bridge() { Stop(); }

void Bridge::Start(Receiver receiver) {
  if (m_reader.joinable() || m_pipe.empty()) {
    return;
  }

  m_receiver = std::move(receiver);
  m_stopping = false;
  m_reader = std::thread([this] { ReadLoop(); });
  m_writer = std::thread([this] { WriteLoop(); });
}

uint64_t Bridge::Send(std::wstring const &command, JsonObject const &fields) {
  const auto requestId = m_id.fetch_add(1);
  JsonObject envelope;
  envelope.Insert(L"version", JsonValue::CreateNumberValue(1));
  envelope.Insert(L"id",
                  JsonValue::CreateNumberValue(static_cast<double>(requestId)));

  JsonObject body;
  body.Insert(L"type", JsonValue::CreateStringValue(command));
  if (fields) {
    for (auto const &field : fields) {
      body.Insert(field.Key(), field.Value());
    }
  }
  envelope.Insert(L"command", body);

  auto payload = to_string(envelope.Stringify());
  if (payload.empty() || payload.size() > kMaximumPayload) {
    Notify(L"Request rejected: message exceeds the 1 MiB protocol limit.");
    return requestId;
  }

  {
    std::lock_guard guard(m_queueMutex);
    if (m_stopping) {
      return requestId;
    }
    m_queue.push_back({requestId, std::move(payload)});
  }
  m_queueChanged.notify_one();
  return requestId;
}

void Bridge::Stop() {
  m_stopping = true;
  m_queueChanged.notify_all();
  {
    std::lock_guard submitGuard(m_ioSubmitMutex);
    std::lock_guard stateGuard(m_stateMutex);
    if (m_handle != INVALID_HANDLE_VALUE) {
      CancelIoEx(m_handle, nullptr);
    }
  }
  m_connected = false;
  if (m_reader.joinable()) {
    m_reader.join();
  }
  if (m_writer.joinable()) {
    m_writer.join();
  }
  {
    std::lock_guard submitGuard(m_ioSubmitMutex);
    std::lock_guard stateGuard(m_stateMutex);
    if (m_handle != INVALID_HANDLE_VALUE) {
      CloseHandle(m_handle);
      m_handle = INVALID_HANDLE_VALUE;
    }
  }
}

bool Bridge::Connect() {
  if (!IsSupportedLocalPipe(m_pipe)) {
    Notify(L"Connection refused: launcher supplied a non-local pipe name.");
    return false;
  }

  const DWORD expectedParent = ParentProcessId();
  const auto deadline = GetTickCount64() + 10'000;
  while (!m_stopping && GetTickCount64() < deadline) {
    auto pipe =
        CreateFileW(m_pipe.c_str(), GENERIC_READ | GENERIC_WRITE, 0, nullptr,
                    OPEN_EXISTING, FILE_FLAG_OVERLAPPED, nullptr);
    if (pipe != INVALID_HANDLE_VALUE) {
      ULONG serverProcess = 0;
      if (expectedParent == 0 ||
          !GetNamedPipeServerProcessId(pipe, &serverProcess) ||
          serverProcess != expectedParent) {
        CloseHandle(pipe);
        Notify(L"Connection refused: named-pipe server is not the launcher "
               L"process.");
        return false;
      }
      bool accepted = false;
      {
        std::lock_guard submitGuard(m_ioSubmitMutex);
        if (!m_stopping) {
          std::lock_guard stateGuard(m_stateMutex);
          m_handle = pipe;
          m_connected = true;
          accepted = true;
        }
      }
      if (!accepted) {
        CloseHandle(pipe);
        return false;
      }
      m_queueChanged.notify_one();
      if (!m_stopping) {
        Notify(L"Connected to launcher.");
      }
      return true;
    }
    Sleep(100);
  }

  if (!m_stopping) {
    Notify(L"Launcher connection timed out after 10 seconds.");
  }
  return false;
}

void Bridge::ReadLoop() {
  ApartmentScope apartment;
  if (!Connect()) {
    m_stopping = true;
    m_connected = false;
    m_queueChanged.notify_all();
    return;
  }

  std::wstring failure = L"Launcher pipe disconnected.";
  while (!m_stopping) {
    std::array<unsigned char, 4> prefix{};
    if (!TransferExact(false, prefix.data(),
                       static_cast<DWORD>(prefix.size()))) {
      break;
    }
    const uint32_t payloadLength = DecodeLength(prefix);
    if (payloadLength == 0 || payloadLength > kMaximumPayload) {
      failure = L"Launcher sent an invalid frame length.";
      break;
    }

    std::string payload(payloadLength, '\0');
    if (!TransferExact(false, payload.data(), payloadLength)) {
      break;
    }

    try {
      auto message = JsonObject::Parse(to_hstring(payload));
      auto version = message.GetNamedNumber(L"version", 0);
      if (version != 1) {
        failure = L"Launcher sent an unsupported protocol version.";
        break;
      }
      m_receiver(message);
    } catch (hresult_error const &) {
      failure = L"Launcher sent an invalid JSON message.";
      break;
    } catch (...) {
      failure = L"Launcher message could not be processed.";
      break;
    }
  }

  SignalStop(std::move(failure));
}

void Bridge::WriteLoop() {
  ApartmentScope apartment;
  while (!m_stopping) {
    Outgoing outgoing{};
    {
      std::unique_lock guard(m_queueMutex);
      m_queueChanged.wait(guard, [this] {
        return m_stopping || (m_connected && !m_queue.empty());
      });
      if (m_stopping) {
        return;
      }
      outgoing = std::move(m_queue.front());
      m_queue.pop_front();
    }

    const auto payloadLength = static_cast<uint32_t>(outgoing.payload.size());
    const auto prefix = EncodeLength(payloadLength);
    if (payloadLength == 0 || payloadLength > kMaximumPayload ||
        !TransferExact(true, prefix.data(),
                       static_cast<DWORD>(prefix.size())) ||
        !TransferExact(true, outgoing.payload.data(), payloadLength)) {
      SignalStop(L"Could not write request to the launcher pipe.");
      return;
    }
  }
}

bool Bridge::TransferExact(bool writing, void const *buffer, DWORD length) {
  auto *cursor = static_cast<unsigned char *>(const_cast<void *>(buffer));
  while (length > 0) {
    HANDLE pipe = INVALID_HANDLE_VALUE;
    OVERLAPPED operation{};
    operation.hEvent = CreateEventW(nullptr, TRUE, FALSE, nullptr);
    if (!operation.hEvent) {
      return false;
    }

    DWORD transferred = 0;
    BOOL started = FALSE;
    DWORD startError = ERROR_SUCCESS;
    {
      std::lock_guard submitGuard(m_ioSubmitMutex);
      if (m_stopping) {
        CloseHandle(operation.hEvent);
        return false;
      }
      {
        std::lock_guard stateGuard(m_stateMutex);
        pipe = m_handle;
      }
      if (pipe == INVALID_HANDLE_VALUE) {
        CloseHandle(operation.hEvent);
        return false;
      }

      if (writing) {
        started = WriteFile(pipe, cursor, length, &transferred, &operation);
      } else {
        started = ReadFile(pipe, cursor, length, &transferred, &operation);
      }
      if (!started) {
        startError = GetLastError();
      }
    }

    if (!started && startError != ERROR_IO_PENDING) {
      CloseHandle(operation.hEvent);
      return false;
    }

    const DWORD waitResult = WaitForSingleObject(operation.hEvent, INFINITE);
    if (waitResult != WAIT_OBJECT_0) {
      CancelIoEx(pipe, &operation);
      DWORD ignored = 0;
      GetOverlappedResult(pipe, &operation, &ignored, TRUE);
      CloseHandle(operation.hEvent);
      return false;
    }

    if (!GetOverlappedResult(pipe, &operation, &transferred, FALSE) ||
        transferred == 0) {
      CloseHandle(operation.hEvent);
      return false;
    }
    CloseHandle(operation.hEvent);
    cursor += transferred;
    length -= transferred;
  }
  return true;
}

void Bridge::SignalStop(std::wstring const &status) {
  if (m_stopping.exchange(true)) {
    return;
  }
  m_queueChanged.notify_all();
  {
    std::lock_guard submitGuard(m_ioSubmitMutex);
    std::lock_guard stateGuard(m_stateMutex);
    if (m_handle != INVALID_HANDLE_VALUE) {
      CancelIoEx(m_handle, nullptr);
    }
  }
  m_connected = false;
  Notify(status);
}

void Bridge::Notify(std::wstring const &status) {
  if (!m_receiver) {
    return;
  }
  JsonObject event;
  event.Insert(L"type", JsonValue::CreateStringValue(L"bridge_status"));
  event.Insert(L"text", JsonValue::CreateStringValue(status));
  event.Insert(L"connected", JsonValue::CreateBooleanValue(m_connected));
  try {
    m_receiver(event);
  } catch (...) {
  }
}

} // namespace winrt::StudioUI::implementation
