import AppKit
import Foundation
import Network
import Observation

/// Maintains a websocket connection to live.bitjita.com, subscribes to a set
/// of channels and reconnects with backoff when the connection drops.
///
/// Dead connections are detected three ways, since a socket can die silently
/// (sleep, Wi-Fi switch) without `receive()` ever failing:
/// - a keepalive ping every `pingInterval`; no pong within `pingTimeout` →
///   reconnect,
/// - the Mac waking from sleep → reconnect immediately,
/// - a network path change (Wi-Fi switch, VPN, cable) → reconnect.
@Observable
final class BitJitaClient {
    enum ConnectionState: Equatable {
        case disconnected
        case connecting
        case connected
    }

    static let url = URL(string: "wss://live.bitjita.com/")!
    static let pingInterval: Duration = .seconds(15)
    static let pingTimeout: Duration = .seconds(10)

    private(set) var state: ConnectionState = .disconnected
    /// When the current connection was established (nil when not connected).
    @ObservationIgnored private(set) var connectedAt: Date?

    /// Called for every `event` message with its channel and the raw JSON message.
    @ObservationIgnored var onEvent: ((_ channel: String, _ message: Data) -> Void)?

    @ObservationIgnored private var runTask: Task<Void, Never>?
    @ObservationIgnored private var socket: URLSessionWebSocketTask?
    @ObservationIgnored private let session = URLSession(configuration: .default)
    /// Everything we want to be subscribed to; re-sent on every reconnect.
    @ObservationIgnored private var channels: Set<String> = []

    @ObservationIgnored private var wakeObserver: NSObjectProtocol?
    @ObservationIgnored private let pathMonitor = NWPathMonitor()
    @ObservationIgnored private var lastPath: NWPath?
    @ObservationIgnored private var pathReconnectTask: Task<Void, Never>?

    init() {
        wakeObserver = NSWorkspace.shared.notificationCenter.addObserver(
            forName: NSWorkspace.didWakeNotification, object: nil, queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated { self?.reconnect() }
        }

        pathMonitor.pathUpdateHandler = { [weak self] path in
            Task { @MainActor in self?.networkPathChanged(path) }
        }
        pathMonitor.start(queue: .main)
    }

    func connect(channels: [String]) {
        disconnect()
        self.channels = Set(channels)
        runTask = Task { [weak self] in
            await self?.run()
        }
    }

    /// Drop the current connection (or backoff wait) and connect again right
    /// away, keeping all subscriptions. Shows "Connecting…" rather than
    /// "Disconnected", so a deliberate reconnect doesn't count as an error.
    func reconnect() {
        guard runTask != nil else { return }
        runTask?.cancel()
        socket?.cancel(with: .goingAway, reason: nil)
        socket = nil
        connectedAt = nil
        state = .connecting
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
        connectedAt = nil
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
            var keepalive: Task<Void, Never>?

            do {
                let initial = channels
                try await send(SubscribeMessage(channels: Array(initial)), on: socket)

                // From here on `subscribe()` sends new channels directly. Any
                // added while the handshake/initial send was in flight would
                // otherwise be lost, so send those now. (No await between
                // these two lines, so nothing can slip in between.)
                state = .connected
                connectedAt = .now
                backoff = .seconds(1)
                let missed = channels.subtracting(initial)
                if !missed.isEmpty {
                    try await send(SubscribeMessage(channels: Array(missed)), on: socket)
                }

                keepalive = Task { await Self.keepAlive(socket) }

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

            keepalive?.cancel()
            connectedAt = nil
            socket.cancel(with: .goingAway, reason: nil)
            guard !Task.isCancelled else { break }
            state = .disconnected

            try? await Task.sleep(for: backoff)
            backoff = min(backoff * 2, .seconds(30))
        }
    }

    /// Pings until cancelled. If a pong doesn't arrive in time, cancels the
    /// socket, which makes `receive()` throw and the run loop reconnect.
    private static func keepAlive(_ socket: URLSessionWebSocketTask) async {
        while !Task.isCancelled {
            try? await Task.sleep(for: pingInterval)
            guard !Task.isCancelled else { return }
            if await !ping(socket, timeout: pingTimeout) {
                guard !Task.isCancelled else { return }
                socket.cancel(with: .goingAway, reason: nil)
                return
            }
        }
    }

    /// True if a pong arrived within `timeout`.
    private static func ping(_ socket: URLSessionWebSocketTask, timeout: Duration) async -> Bool {
        await withTaskGroup(of: Bool.self) { group in
            group.addTask {
                await withCheckedContinuation { continuation in
                    let once = ResumeOnce()
                    socket.sendPing { error in
                        if once.claim() { continuation.resume(returning: error == nil) }
                    }
                }
            }
            group.addTask {
                try? await Task.sleep(for: timeout)
                return false
            }
            let result = await group.next() ?? false
            group.cancelAll()
            return result
        }
    }

    /// Reconnect when the network actually changes (not on the first report,
    /// and not when it goes offline: the ping/backoff handles that). Debounced
    /// because a change often comes as a burst of updates.
    private func networkPathChanged(_ path: NWPath) {
        defer { lastPath = path }
        guard let previous = lastPath, path.status == .satisfied else { return }
        let changed = previous.status != .satisfied
            || previous.availableInterfaces.map(\.name) != path.availableInterfaces.map(\.name)
        guard changed else { return }

        pathReconnectTask?.cancel()
        pathReconnectTask = Task { [weak self] in
            try? await Task.sleep(for: .seconds(2))
            guard !Task.isCancelled else { return }
            self?.reconnect()
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

/// Guards a continuation against being resumed twice.
nonisolated private final class ResumeOnce: @unchecked Sendable {
    private let lock = NSLock()
    private var claimed = false

    func claim() -> Bool {
        lock.lock()
        defer { lock.unlock() }
        if claimed { return false }
        claimed = true
        return true
    }
}
