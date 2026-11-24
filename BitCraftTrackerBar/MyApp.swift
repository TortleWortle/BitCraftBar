import SwiftUI

@main struct MyApp: App {
    @State private var model = AppModel()

    var body: some Scene {
        MenuBarExtra {
            StatusPanelView(model: model)
        } label: {
            MenuBarLabel(
                isLoading: model.isLoading,
                isOffline: model.isOffline,
                status: model.monitor.overall,
                staminaFraction: model.stamina.fraction,
                skillName: model.experience.lastSkillName,
                craftPercent: model.activeCraft?.percentString,
                plantSummary: model.plants.menuBarSummary,
                issues: model.monitor.activeIssues
            )
        }
        .menuBarExtraStyle(.window)
    }
}
