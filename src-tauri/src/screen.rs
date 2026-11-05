//! Съёмка экрана через xcap. Скриншоты делаются только по запросу пользователя
//! («посмотри на экран»), не сохраняются, передаются в vision-модель и удаляются.

/// Снимает основной монитор → PNG-байты.
pub async fn capture_png() -> Result<Vec<u8>, String> {
    let monitors = xcap::Monitor::all().map_err(|e| format!("не удалось получить мониторы: {}", e))?;
    let monitor = monitors
        .into_iter()
        .find(|m| m.is_primary().unwrap_or(false))
        .or_else(|| xcap::Monitor::all().ok().and_then(|all| all.into_iter().next()))
        .ok_or("нет мониторов")?;
    let image = monitor.capture_image().map_err(|e| e.to_string())?;
    let mut png = Vec::new();
    image
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|e| format!("не удалось закодировать PNG: {}", e))?;
    Ok(png)
}
