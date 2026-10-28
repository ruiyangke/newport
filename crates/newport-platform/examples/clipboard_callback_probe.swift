// Diagnostic for AppKit callback threading and deferred completion, using only
// disposable named pasteboards. Compile with swiftc; invoke an owner as
//   clipboard_callback_probe main|worker|deferred org.newport.test.lazy.UUID
// and a separate consumer as
//   clipboard_callback_probe read org.newport.test.lazy.UUID
// Owners exit after three seconds. They never open the general pasteboard.
import AppKit
import Foundation

final class Provider: NSObject, NSPasteboardItemDataProvider {
    let deferred: Bool
    init(deferred: Bool) { self.deferred = deferred }
    func pasteboard(_ pasteboard: NSPasteboard?, item: NSPasteboardItem,
                    provideDataForType type: NSPasteboard.PasteboardType) {
        print("CALLBACK main=\(Thread.isMainThread)")
        fflush(stdout)
        if deferred {
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.35) {
                let written = item.setData(Data("lazy bytes".utf8), forType: type)
                print("SET \(written)")
                fflush(stdout)
            }
        } else {
            Thread.sleep(forTimeInterval: 0.35)
            item.setData(Data("lazy bytes".utf8), forType: type)
        }
    }
}

let args = CommandLine.arguments
precondition(args.count == 3)
let mode = args[1], name = args[2]
precondition(name.hasPrefix("org.newport.test.lazy."))
precondition(["read", "main", "worker", "deferred"].contains(mode))
if mode == "read" {
    print("READ \(NSPasteboard(name: NSPasteboard.Name(name)).data(forType: .string)?.count ?? -1)")
} else {
    NSApplication.shared.setActivationPolicy(.prohibited)
    let deadline = Date().addingTimeInterval(3)
    func own() {
        let board = NSPasteboard(name: NSPasteboard.Name(name))
        let provider = Provider(deferred: mode == "deferred")
        let item = NSPasteboardItem()
        precondition(item.setDataProvider(provider, forTypes: [.string]))
        board.clearContents()
        precondition(board.writeObjects([item]))
        print("READY main=\(Thread.isMainThread)")
        fflush(stdout)
        while Date() < deadline {
            _ = RunLoop.current.run(mode: .default, before: Date().addingTimeInterval(0.01))
        }
        withExtendedLifetime(provider) {}
    }
    if mode == "worker" { Thread.detachNewThread { own() } }
    var previous = Date(), maximum: TimeInterval = 0
    let timer = Timer.scheduledTimer(withTimeInterval: 0.02, repeats: true) { _ in
        let now = Date()
        maximum = max(maximum, now.timeIntervalSince(previous))
        previous = now
    }
    if mode != "worker" { own() }
    else {
        while Date() < deadline {
            _ = RunLoop.current.run(mode: .default, before: Date().addingTimeInterval(0.01))
        }
    }
    timer.invalidate()
    print("UI_GAP \(maximum)")
    let board = NSPasteboard(name: NSPasteboard.Name(name))
    board.clearContents()
    board.releaseGlobally()
}
