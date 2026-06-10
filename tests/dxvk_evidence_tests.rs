use aurelia::infra::logging::wine_capture::classify_graphics_evidence;

#[test]
fn test_new_dxvk_patterns() {
    let line1 = "info:  Game: BatmanOrigins.exe";
    assert!(classify_graphics_evidence(line1).unwrap().contains("DXVK Detected"));

    let line2 = "info:  D3D11InternalCreateDevice: Requested feature level 0xb000";
    assert!(classify_graphics_evidence(line2).unwrap().contains("DXVK Detected"));

    let line3 = "info:  Presenter: Actual swapchain properties: 1920x1080, format DXGI_FORMAT_B8G8R8A8_UNORM, mailbox 0";
    assert!(classify_graphics_evidence(line3).unwrap().contains("DXVK Detected"));

    let line4 = "info:  Vulkan: Found vkGetInstanceProcAddr at 0x7f8a12345678";
    assert!(classify_graphics_evidence(line4).unwrap().contains("DXVK Detected"));
}
