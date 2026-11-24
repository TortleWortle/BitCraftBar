import SwiftUI

/// Shows the tracked character's name, with a username search to switch.
struct CharacterPicker: View {
    let model: AppModel

    @State private var isSearching = false
    @State private var query = ""
    @State private var results: [BitJitaAPI.PlayerSummary] = []
    @State private var isLoading = false
    @State private var error: String?
    @FocusState private var fieldFocused: Bool

    private static let rowHeight: CGFloat = 22
    /// Rows visible before the list scrolls.
    private static let visibleRows = 10

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                Text("Character").foregroundStyle(.secondary)
                Text(model.username ?? "…").fontWeight(.medium)
                Spacer()
                Button(isSearching ? "Cancel" : "Change") {
                    isSearching.toggle()
                    if isSearching {
                        query = ""
                        results = []
                        error = nil
                        fieldFocused = true
                    }
                }
            }

            if isSearching {
                TextField("Search username", text: $query)
                    .textFieldStyle(.roundedBorder)
                    .focused($fieldFocused)
                    .onSubmit {
                        if let first = results.first { select(first) }
                    }

                searchResults
            }
        }
        .font(.callout)
        // Debounced search; restarts (and cancels the previous one) on every edit.
        .task(id: query) {
            guard isSearching else { return }
            let trimmed = query.trimmingCharacters(in: .whitespacesAndNewlines)
            guard trimmed.count >= 2 else {
                results = []
                error = nil
                return
            }
            try? await Task.sleep(for: .milliseconds(300))
            guard !Task.isCancelled else { return }
            isLoading = true
            defer { isLoading = false }
            do {
                let found = try await BitJitaAPI.searchPlayers(trimmed)
                guard !Task.isCancelled else { return }
                results = found
                error = nil
            } catch {
                guard !Task.isCancelled else { return }
                results = []
                self.error = "Search failed"
            }
        }
    }

    @ViewBuilder
    private var searchResults: some View {
        let trimmed = query.trimmingCharacters(in: .whitespacesAndNewlines)
        if trimmed.count < 2 {
            Text("Type at least 2 characters")
                .font(.caption)
                .foregroundStyle(.secondary)
        } else if let error {
            Text(error).font(.caption).foregroundStyle(.red)
        } else if results.isEmpty {
            Text(isLoading ? "Searching…" : "No players found")
                .font(.caption)
                .foregroundStyle(.secondary)
        } else {
            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    ForEach(results) { player in
                        Button { select(player) } label: {
                            HStack(spacing: 6) {
                                Circle()
                                    .fill(player.signedIn == true ? Color.green : Color.secondary.opacity(0.4))
                                    .frame(width: 6, height: 6)
                                Text(player.username)
                                Spacer()
                                if player.entityId == model.userId {
                                    Image(systemName: "checkmark").foregroundStyle(.secondary)
                                }
                            }
                            .frame(height: Self.rowHeight)
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                    }
                }
            }
            // A ScrollView in a size-to-fit popover collapses to almost no
            // height (maxHeight only caps it), so give it an explicit height.
            .frame(height: Self.rowHeight * CGFloat(min(results.count, Self.visibleRows)))

            // The server returns at most 50 matches (alphabetical, no paging),
            // so the wanted name may be cut off for short queries.
            if results.count >= 50 {
                Text("Showing first 50, type more of the name")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
    }

    private func select(_ player: BitJitaAPI.PlayerSummary) {
        model.selectPlayer(player)
        isSearching = false
        query = ""
        results = []
    }
}
