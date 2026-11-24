import Foundation
import Observation

/// Maintains a websocket connection to live.bitjita.com, subscribes to a set
/// of channels and reconnects with backoff when the connection drops.
@Observable
final class BitJitaClient {
    enum ConnectionState: Equatable {
        case disconnected
        case connecting
        case connected
    }

    static let url = URL(string: "wss://live.bitjita.com/")!

    private(set) var state: ConnectionState = .disconnected

    /// Called for every `event` message with its channel and the raw JSON message.
    @ObservationIgnored var onEvent: ((_ channel: String, _ message: Data) -> Void)?

    @ObservationIgnored private var runTask: Task<Void, Never>?
    @ObservationIgnored private var socket: URLSessionWebSocketTask?
    @ObservationIgnored private let session = URLSession(configuration: .default)
    /// Everything we want to be subscribed to; re-sent on every reconnect.
    @ObservationIgnored private var channels: Set<String> = []

    func connect(channels: [String]) {
        disconnect()
        self.channels = Set(channels)
        runTask = Task { [weak self] in
            await self?.run()
        }
    }

    /// Adds channels to the subscription, sending them immediately if connected.
    func subscribe(_ newChannels: [String]) {
        let added = Set(newChannels).subtracting(channels)
        guard !added.isEmpty else { return }
        channels.formUnion(added)
        if state == .connected, let socket {
            Task { try? await send(SubscribeMessage(channels: Array(added)), on: socket) }
        }
    }

    func disconnect() {
        runTask?.cancel()
        runTask = nil
        socket?.cancel(with: .goingAway, reason: nil)
        socket = nil
        state = .disconnected
    }

    private func send(_ message: SubscribeMessage, on socket: URLSessionWebSocketTask) async throws {
        let data = try JSONEncoder().encode(message)
        try await socket.send(.string(String(decoding: data, as: UTF8.self)))
    }

    private func run() async {
        var backoff: Duration = .seconds(1)

        while !Task.isCancelled {
            state = .connecting
            let socket = session.webSocketTask(with: Self.url)
            self.socket = socket
            socket.resume()

            do {
                let initial = channels
                try await send(SubscribeMessage(channels: Array(initial)), on: socket)

                // From here on `subscribe()` sends new channels directly. Any
                // added while the handshake/initial send was in flight would
                // otherwise be lost, so send those now. (No await between
                // these two lines, so nothing can slip in between.)
                state = .connected
                backoff = .seconds(1)
                let missed = channels.subtracting(initial)
                if !missed.isEmpty {
                    try await send(SubscribeMessage(channels: Array(missed)), on: socket)
                }

                while !Task.isCancelled {
                    let message = try await socket.receive()
                    switch message {
                    case .string(let text): handle(Data(text.utf8))
                    case .data(let data): handle(data)
                    @unknown default: break
                    }
                }
            } catch {
                // Fall through to reconnect.
            }

            socket.cancel(with: .goingAway, reason: nil)
            guard !Task.isCancelled else { break }
            state = .disconnected

            try? await Task.sleep(for: backoff)
            backoff = min(backoff * 2, .seconds(30))
        }
    }

    private func handle(_ data: Data) {
        guard let envelope = try? JSONDecoder().decode(IncomingEnvelope.self, from: data),
              envelope.type == "event",
              let channel = envelope.channel
        else { return }
        onEvent?(channel, data)
    }
}
