// Perf probe for one app window, picked by PID (seven agents run apps at once):
//   cap list <pid>                                   window id and bounds
//   cap shot <pid> <out.png>                         a screenshot of that window (screencapture -l)
//   cap click <pid> <x> <y> <secs> <cx> <cy> <cw> <ch>
//       clicks window point (x,y), records the crop (cx,cy,cw,ch, window points) with ScreenCaptureKit for secs and
//       prints when the crop first changed and when it last changed (= the first full frame of what the click opened)
//   cap scroll <pid> <x> <y> <secs> <dy> <cx> <cy> <cw> <ch>
//       a pixel scroll of dy every 1/120 s at (x,y) for secs; prints how many distinct frames the crop showed and the
//       frame intervals (median, p95, max)
import AppKit
import ScreenCaptureKit
import CoreMedia
import CoreImage

let a = CommandLine.arguments
let pid = pid_t(a[2])!
func windowInfo() -> (CGWindowID, CGRect) {
    let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as! [[String: Any]]
    let w = list.filter { ($0[kCGWindowOwnerPID as String] as? pid_t) == pid && ($0[kCGWindowLayer as String] as? Int) == 0 }
        .max { r($0).width * r($0).height < r($1).width * r($1).height }
    guard let w else { fputs("no window for \(pid)\n", stderr); exit(1) }
    return (w[kCGWindowNumber as String] as! CGWindowID, r(w))
}
func r(_ w: [String: Any]) -> CGRect { CGRect(dictionaryRepresentation: w[kCGWindowBounds as String] as! CFDictionary)! }
func now() -> Double { CMClockGetTime(CMClockGetHostTimeClock()).seconds }

final class Rec: NSObject, SCStreamOutput {
    var frames: [(Double, UInt64)] = []
    var last = [UInt8](); var id: UInt64 = 0
    let lock = NSLock()
    func stream(_ s: SCStream, didOutputSampleBuffer sb: CMSampleBuffer, of type: SCStreamOutputType) {
        guard type == .screen,
              let att = (CMSampleBufferGetSampleAttachmentsArray(sb, createIfNecessary: false) as? [[SCStreamFrameInfo: Any]])?.first,
              let raw = att[.status] as? Int, SCFrameStatus(rawValue: raw) == .complete,
              let pb = CMSampleBufferGetImageBuffer(sb) else { return }
        CVPixelBufferLockBaseAddress(pb, .readOnly)
        let base = CVPixelBufferGetBaseAddress(pb)!.assumingMemoryBound(to: UInt8.self)
        let bpr = CVPixelBufferGetBytesPerRow(pb), h = CVPixelBufferGetHeight(pb), w = CVPixelBufferGetWidth(pb)
        // A frame counts as new only when some sampled byte moved by more than 12: the compositor shifts colours by
        // one or two steps on its own (seen ~1.5 s after a change), which isn't the app drawing.
        var sample = [UInt8](); sample.reserveCapacity(w * h / 4)
        for y in stride(from: 0, to: h, by: 2) { let row = base + y * bpr; for x in stride(from: 0, to: w * 4, by: 4) { sample.append(row[x + 1]) } }
        CVPixelBufferUnlockBaseAddress(pb, .readOnly)
        let t = CMSampleBufferGetPresentationTimeStamp(sb).seconds
        lock.lock()
        let changed = last.count != sample.count || zip(last, sample).contains { abs(Int($0) - Int($1)) > 12 }
        if changed { id += 1; last = sample }
        frames.append((t, id)); lock.unlock()
        if changed, let dir = ProcessInfo.processInfo.environment["DUMP"] {   // each distinct frame, for checking what changed
            let ci = CIImage(cvPixelBuffer: pb)
            if let cg = CIContext().createCGImage(ci, from: ci.extent) {
                let rep = NSBitmapImageRep(cgImage: cg)
                try? rep.representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: "\(dir)/\(String(format: "%.3f", t)).png"))
            }
        }
    }
}

func record(crop: CGRect, secs: Double, act: @escaping (CGRect) -> Void) async throws -> (Double, [(Double, UInt64)]) {
    let (id, bounds) = windowInfo()
    let content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: true)
    guard let win = content.windows.first(where: { $0.windowID == id }) else { fputs("SCK has no window \(id)\n", stderr); exit(1) }
    let cfg = SCStreamConfiguration()
    cfg.sourceRect = crop
    cfg.width = Int(crop.width); cfg.height = Int(crop.height)
    cfg.minimumFrameInterval = CMTime(value: 1, timescale: 240)
    cfg.queueDepth = 8
    cfg.showsCursor = false
    let rec = Rec()
    let s = SCStream(filter: SCContentFilter(desktopIndependentWindow: win), configuration: cfg, delegate: nil)
    try s.addStreamOutput(rec, type: .screen, sampleHandlerQueue: DispatchQueue(label: "cap"))
    try await s.startCapture()
    try await Task.sleep(for: .milliseconds(600))
    let t0 = now()
    act(bounds)
    try await Task.sleep(for: .seconds(secs))
    try await s.stopCapture()
    rec.lock.lock(); defer { rec.lock.unlock() }
    return (t0, rec.frames)
}

func activate() async {
    NSRunningApplication(processIdentifier: pid)?.activate()
    try? await Task.sleep(for: .milliseconds(400))
}
func crop(_ i: Int) -> CGRect { CGRect(x: Double(a[i])!, y: Double(a[i + 1])!, width: Double(a[i + 2])!, height: Double(a[i + 3])!) }

switch a[1] {
case "list":
    let (id, b) = windowInfo(); print(id, Int(b.minX), Int(b.minY), Int(b.width), Int(b.height))
case "shot":
    let (id, _) = windowInfo()
    let p = Process(); p.executableURL = URL(fileURLWithPath: "/usr/sbin/screencapture"); p.arguments = ["-o", "-x", "-l", "\(id)", a[3]]
    try p.run(); p.waitUntilExit()
case "click":
    await activate()
    let x = Double(a[3])!, y = Double(a[4])!
    let (t0, f) = try await record(crop: crop(6), secs: Double(a[5])!) { b in
        let pt = CGPoint(x: b.minX + x, y: b.minY + y)
        CGEvent(mouseEventSource: nil, mouseType: .mouseMoved, mouseCursorPosition: pt, mouseButton: .left)?.post(tap: .cghidEventTap)
        usleep(30_000)
        CGEvent(mouseEventSource: nil, mouseType: .leftMouseDown, mouseCursorPosition: pt, mouseButton: .left)?.post(tap: .cghidEventTap)
        CGEvent(mouseEventSource: nil, mouseType: .leftMouseUp, mouseCursorPosition: pt, mouseButton: .left)?.post(tap: .cghidEventTap)
    }
    let before = f.last(where: { $0.0 < t0 })?.1 ?? f.first?.1
    let after = f.filter { $0.0 >= t0 }
    var changes: [Double] = []; var prev = before
    for (t, h) in after where h != prev { changes.append(t - t0); prev = h }
    print(String(format: "click first-change=%.3f last-change=%.3f changes=%d frames=%d", changes.first ?? -1, changes.last ?? -1, changes.count, f.count))
case "scroll":
    await activate()
    let x = Double(a[3])!, y = Double(a[4])!, secs = Double(a[5])!, dy = Int32(a[6])!
    let (t0, f) = try await record(crop: crop(7), secs: secs + 0.3) { b in
        let pt = CGPoint(x: b.minX + x, y: b.minY + y)
        CGEvent(mouseEventSource: nil, mouseType: .mouseMoved, mouseCursorPosition: pt, mouseButton: .left)?.post(tap: .cghidEventTap)
        if let ms = ProcessInfo.processInfo.environment["SETTLE_MS"] { usleep(UInt32(ms)! * 1000) }   // let hover effects draw before the scroll starts
        Thread.detachNewThread {
            let end = now() + secs
            while now() < end {
                CGEvent(scrollWheelEvent2Source: nil, units: .pixel, wheelCount: 1, wheel1: -dy, wheel2: 0, wheel3: 0)?.post(tap: .cghidEventTap)
                usleep(UInt32(ProcessInfo.processInfo.environment["EVERY_US"] ?? "8333")!)
            }
        }
    }
    var times: [Double] = []; var prev: UInt64? = nil
    for (t, h) in f where t >= t0 && t <= t0 + secs { if h != prev { times.append(t) }; prev = h }
    let iv = zip(times.dropFirst(), times).map { ($0 - $1) * 1000 }.sorted()
    let q = { (p: Double) in iv.isEmpty ? -1 : iv[min(iv.count - 1, Int(Double(iv.count) * p))] }
    let gaps = zip(times.dropFirst(), times).map { ($0 - $1) * 1000 }
    let maxAt = gaps.indices.max { gaps[$0] < gaps[$1] }.map { times[$0] - t0 } ?? -1
    print(String(format: "scroll distinct=%d fps=%.1f p50=%.1fms p95=%.1fms max=%.1fms max-at=%.2fs first-frame=%.3fs over33ms=%d", times.count, Double(times.count) / secs, q(0.5), q(0.95), iv.last ?? -1, maxAt, (times.first ?? t0) - t0, gaps.filter { $0 > 33 }.count))
default: print("?")
}
