use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use uuid::Uuid;

/// 图片存储管理器
#[derive(Clone)]
pub struct ImageStorage {
    base_dir: PathBuf,
}

impl ImageStorage {
    pub fn new(base_dir: PathBuf) -> Self {
        Self { base_dir }
    }

    /// 保存图片到存储目录，返回存储后的相对路径
    pub async fn save(&self, source_path: &Path) -> Result<String> {
        // 确保目录存在
        tokio::fs::create_dir_all(&self.base_dir)
            .await
            .with_context(|| format!("创建图片存储目录失败: {}", self.base_dir.display()))?;

        let extension = source_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("png");

        let file_name = format!("{}.{}", Uuid::new_v4(), extension);
        let dest_path = self.base_dir.join(&file_name);

        tokio::fs::copy(source_path, &dest_path)
            .await
            .with_context(|| {
                format!(
                    "复制图片失败: {} -> {}",
                    source_path.display(),
                    dest_path.display()
                )
            })?;

        tracing::info!("图片已保存: {}", dest_path.display());
        Ok(file_name)
    }

    /// 根据相对路径获取完整路径
    pub fn full_path(&self, relative_path: &str) -> PathBuf {
        self.base_dir.join(relative_path)
    }

    /// 读取图片为 base64
    pub async fn read_base64(&self, relative_path: &str) -> Result<String> {
        let full_path = self.full_path(relative_path);
        let bytes = tokio::fs::read(&full_path)
            .await
            .with_context(|| format!("读取图片失败: {}", full_path.display()))?;
        Ok(base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &bytes))
    }
}
