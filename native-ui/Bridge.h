#pragma once

#include "pch.h"
#include <condition_variable>
#include <deque>

namespace winrt::StudioUI::implementation {

struct Bridge {
  using Receiver =
      std::function<void(winrt::Windows::Data::Json::JsonObject const &)>;

  Bridge();
  ~Bridge();

  void Start(Receiver receiver);
  bool IsConfigured() const noexcept { return !m_pipe.empty(); }
  uint64_t Send(std::wstring const &command,
                winrt::Windows::Data::Json::JsonObject const &fields = nullptr);
  void Stop();

private:
  struct Outgoing {
    uint64_t id;
    std::string payload;
  };

  void ReadLoop();
  void WriteLoop();
  void Notify(std::wstring const &status);
  void SignalStop(std::wstring const &status);
  bool TransferExact(bool writing, void const *buffer, DWORD length);
  bool Connect();

  std::wstring m_pipe;
  Receiver m_receiver;
  std::thread m_reader;
  std::thread m_writer;
  std::atomic_bool m_stopping{false};
  std::atomic_bool m_connected{false};
  // Prevents Stop from canceling before a worker submits its next I/O.
  std::mutex m_ioSubmitMutex;
  std::mutex m_stateMutex;
  std::mutex m_queueMutex;
  std::condition_variable m_queueChanged;
  std::deque<Outgoing> m_queue;
  HANDLE m_handle{INVALID_HANDLE_VALUE};
  std::atomic_uint64_t m_id{1};
};

} // namespace winrt::StudioUI::implementation
