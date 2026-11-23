import UIKit
import Capacitor

@objc(ImageSaverPlugin)
public class ImageSaverPlugin: CAPPlugin, CAPBridgedPlugin {
    public let identifier = "ImageSaverPlugin"
    public let jsName = "ImageSaver"
    public let pluginMethods: [CAPPluginMethod] = [
        CAPPluginMethod(name: "save", returnType: CAPPluginReturnPromise)
    ]

    @objc func save(_ call: CAPPluginCall) {
        guard let encoded = call.getString("data"), !encoded.isEmpty,
              encoded.utf8.count <= 40 * 1024 * 1024,
              let data = Data(base64Encoded: encoded),
              let name = call.getString("name"),
              name.range(of: "^image-[a-f0-9]{8}\\.(png|jpg|webp)$", options: .regularExpression) != nil,
              let mime = call.getString("mime"),
              ["image/png", "image/jpeg", "image/webp"].contains(mime) else {
            call.reject("图片参数无效")
            return
        }
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString, isDirectory: true)
        let file = directory.appendingPathComponent(name)
        do {
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
            try data.write(to: file, options: .atomic)
        } catch {
            try? FileManager.default.removeItem(at: directory)
            call.reject("保存图片失败：\(error.localizedDescription)")
            return
        }
        DispatchQueue.main.async {
            guard let controller = self.bridge?.viewController, controller.presentedViewController == nil else {
                try? FileManager.default.removeItem(at: directory)
                call.reject("当前无法打开图片保存窗口，请关闭其它窗口后重试")
                return
            }
            let sheet = UIActivityViewController(activityItems: [file], applicationActivities: nil)
            sheet.completionWithItemsHandler = { _, completed, _, error in
                try? FileManager.default.removeItem(at: directory)
                if let error = error { call.reject("保存图片失败：\(error.localizedDescription)") }
                else { call.resolve(["saved": completed, "canceled": !completed]) }
            }
            if let popover = sheet.popoverPresentationController {
                popover.sourceView = controller.view
                popover.sourceRect = CGRect(x: controller.view.bounds.midX, y: controller.view.bounds.midY, width: 1, height: 1)
                popover.permittedArrowDirections = []
            }
            controller.present(sheet, animated: true)
        }
    }
}

@objc(ExternalUrlPlugin)
public class ExternalUrlPlugin: CAPPlugin, CAPBridgedPlugin {
    public let identifier = "ExternalUrlPlugin"
    public let jsName = "ExternalUrl"
    public let pluginMethods: [CAPPluginMethod] = [
        CAPPluginMethod(name: "open", returnType: CAPPluginReturnPromise)
    ]

    @objc func open(_ call: CAPPluginCall) {
        guard let raw = call.getString("url")?.trimmingCharacters(in: .whitespacesAndNewlines),
              let url = URL(string: raw),
              let scheme = url.scheme?.lowercased(),
              scheme == "http" || scheme == "https" else {
            call.reject("Invalid external URL")
            return
        }
        DispatchQueue.main.async {
            UIApplication.shared.open(url, options: [:]) { opened in
                if opened { call.resolve() }
                else { call.reject("Failed to open external URL") }
            }
        }
    }
}

class AppBridgeViewController: CAPBridgeViewController {
    override func capacitorDidLoad() {
        bridge?.registerPluginInstance(ImageSaverPlugin())
        bridge?.registerPluginInstance(ExternalUrlPlugin())
    }
}
