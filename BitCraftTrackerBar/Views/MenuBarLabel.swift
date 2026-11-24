import AppKit
import SwiftUI

/// The item shown in the macOS menu bar:
/// a status-colored ring around a stamina pie in the same color, then the last skill,
/// craft percentage, plant summary (after a dash) and any active issues.
struct MenuBarLabel: View {
    let status: HealthStatus
    let staminaFraction: Double?
    let skillName: String?
    let craftPercent: String?
    let plantSummary: String?
    let issues: [StatusMonitor.Report]

    var body: some View {
        // Menu bar labels are template-rendered (monochrome) and ignore most
        // SwiftUI layout, so draw everything into one non-template NSImage.
        Image(nsImage: Self.render(
            status: status,
            staminaFraction: staminaFraction,
            text: Self.text(skillName: skillName, craftPercent: craftPercent, plantSummary: plantSummary, issues: issues)
        ))
    }

    static func text(skillName: String?, craftPercent: String?, plantSummary: String? = nil, issues: [StatusMonitor.Report]) -> NSAttributedString {
        let font = NSFont.menuBarFont(ofSize: 0)
        let result = NSMutableAttributedString()
        let separator = NSAttributedString(
            string: " · ",
            attributes: [.font: font, .foregroundColor: NSColor.secondaryLabelColor]
        )
        func append(_ string: String, _ color: NSColor, font: NSFont = font, separated: Bool = true) {
            if separated, result.length > 0 { result.append(separator) }
            result.append(NSAttributedString(string: string, attributes: [.font: font, .foregroundColor: color]))
        }

        if let skillName {
            append(skillName, .labelColor)
        }
        if let craftPercent {
            // Directly after the skill name, e.g. "Leatherworking 22%".
            append(
                (result.length > 0 ? " " : "") + craftPercent,
                .labelColor,
                font: .monospacedDigitSystemFont(ofSize: font.pointSize, weight: .regular),
                separated: false
            )
        }
        if let plantSummary {
            // e.g. "Leatherworking 22% — T2 grown; T1 needs water"
            append((result.length > 0 ? " — " : "") + plantSummary, .labelColor, separated: false)
        }
        for issue in issues {
            append(issue.message, issue.status.nsColor)
        }
        return result
    }

    static func render(status: HealthStatus, staminaFraction: Double?, text: NSAttributedString) -> NSImage {
        let diameter: CGFloat = 16
        let ringWidth: CGFloat = 1.5
        let gap: CGFloat = 1.5
        let spacing: CGFloat = 5
        let textSize = text.length > 0 ? text.size() : .zero
        let height = max(diameter, ceil(textSize.height))
        let width = diameter + (text.length > 0 ? spacing + ceil(textSize.width) : 0)

        // Drawing handler runs per appearance, so label colors follow the
        // light/dark menu bar.
        let image = NSImage(size: NSSize(width: width, height: height), flipped: false) { _ in
            let outer = NSRect(x: 0, y: (height - diameter) / 2, width: diameter, height: diameter)

            let color = status.nsColor

            // Status ring.
            let ring = NSBezierPath(ovalIn: outer.insetBy(dx: ringWidth / 2, dy: ringWidth / 2))
            ring.lineWidth = ringWidth
            color.setStroke()
            ring.stroke()

            // Stamina pie inside the ring: faint track, then a wedge starting
            // at 12 o'clock going counter-clockwise. Without stamina data, a solid dot.
            let inner = outer.insetBy(dx: ringWidth + gap, dy: ringWidth + gap)
            (staminaFraction == nil ? color : color.withAlphaComponent(0.3)).setFill()
            NSBezierPath(ovalIn: inner).fill()

            if let staminaFraction, staminaFraction > 0 {
                let center = NSPoint(x: inner.midX, y: inner.midY)
                let radius = inner.width / 2
                color.setFill()
                if staminaFraction >= 1 {
                    NSBezierPath(ovalIn: inner).fill()
                } else {
                    let wedge = NSBezierPath()
                    wedge.move(to: center)
                    wedge.appendArc(
                        withCenter: center,
                        radius: radius,
                        startAngle: 90,
                        endAngle: 90 + 360 * staminaFraction,
                        clockwise: false
                    )
                    wedge.close()
                    wedge.fill()
                }
            }

            if text.length > 0 {
                text.draw(at: NSPoint(x: diameter + spacing, y: (height - textSize.height) / 2))
            }
            return true
        }
        image.isTemplate = false
        return image
    }
}
