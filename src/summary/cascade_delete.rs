use std::path::PathBuf;

use anyhow::Result;

use crate::db::repository::Repository;

/// Result of a cascade-delete operation on a summary.
#[derive(Debug)]
pub struct CascadeDeleteResult {
    pub summary_id: String,
    pub deleted_summary_images: u64,
    pub deleted_practice_sets: u64,
    pub deleted_summary: bool,
    /// Number of generated image files actually removed from disk.
    pub deleted_generated_image_files: u64,
}

/// Cascade-delete a summary and all its related records.
///
/// This deletes:
/// - All `summary_images` rows (and best-effort deletes their generated image files)
/// - All `practice_sets` rows (but **not** their PDF files, which may be user-chosen paths)
/// - The `summaries` row itself
///
/// Returns a not-found error if the summary does not exist.
pub async fn cascade_delete_summary(
    repository: &Repository,
    summary_id: &str,
    generated_image_dir: PathBuf,
) -> Result<CascadeDeleteResult> {
    // 1. Verify the summary exists
    let summary = repository.get_summary(summary_id).await?;
    if summary.is_none() {
        anyhow::bail!("总结记录不存在: {}", summary_id);
    }

    // 2. Collect summary_images (to delete their files)
    let summary_images = repository.list_summary_images(summary_id).await?;
    let mut deleted_files: u64 = 0;
    for img in &summary_images {
        let file_path = generated_image_dir.join(&img.image_path);
        match tokio::fs::remove_file(&file_path).await {
            Ok(()) => {
                tracing::info!(path = %file_path.display(), "已删除生成图片文件");
                deleted_files += 1;
            }
            Err(e) => {
                tracing::warn!(
                    path = %file_path.display(),
                    error = %e,
                    "删除生成图片文件失败（忽略）"
                );
            }
        }
    }

    // 3. Delete summary_images rows
    let deleted_summary_images = repository
        .delete_summary_images_by_summary(summary_id)
        .await?;

    // 4. Delete practice_sets rows (do NOT delete PDF files)
    let deleted_practice_sets = repository
        .delete_practice_sets_by_summary(summary_id)
        .await?;

    // 5. Delete the summary row
    repository.delete_summary(summary_id).await?;

    Ok(CascadeDeleteResult {
        summary_id: summary_id.to_string(),
        deleted_summary_images,
        deleted_practice_sets,
        deleted_summary: true,
        deleted_generated_image_files: deleted_files,
    })
}
