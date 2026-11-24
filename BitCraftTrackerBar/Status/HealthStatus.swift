import SwiftUI

/// Overall health level shown by the menu bar indicator.
/// Ordered by severity so the worst one can be picked with `max()`.
enum HealthStatus: Int, Comparable, CaseIterable {
    case healthy
    case warning
    case alert

    static func < (lhs: HealthStatus, rhs: HealthStatus) -> Bool {
        lhs.rawValue < rhs.rawValue
    }

    var color: Color {
        switch self {
        case .healthy: .green
        case .warning: .orange
        case .alert: .red
        }
    }

    var nsColor: NSColor {
        switch self {
        case .healthy: .systemGreen
        case .warning: .systemOrange
        case .alert: .systemRed
        }
    }
}
