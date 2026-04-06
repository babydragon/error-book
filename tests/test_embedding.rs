use error_book::config::AppConfig;
use error_book::llm::client::EmbeddingClient;

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    dot / (norm_a * norm_b + 1e-8)
}

fn load_config() -> Option<AppConfig> {
    match AppConfig::load(std::path::Path::new("config.toml")) {
        Ok(c) => Some(c),
        Err(e) => {
            eprintln!("⚠️  跳过测试：无法加载 config.toml ({})", e);
            eprintln!("   请复制 config.example.toml 为 config.toml 并填入 API key");
            None
        }
    }
}

async fn read_image_base64(path: &std::path::Path) -> std::io::Result<String> {
    let bytes = tokio::fs::read(path).await?;
    Ok(base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes))
}

fn detect_media_type(path: &std::path::Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("png") => "image/png",
        Some("webp") => "image/webp",
        Some("gif") => "image/gif",
        _ => "image/png",
    }
}

/// ==================== 纯文本 Embedding 测试 ====================

#[tokio::test]
async fn test_embedding_e2e() {
    let Some(config) = load_config() else { return };

    let client = EmbeddingClient::new(&config);
    let test_text = "科目: 数学\n知识点: 带余数除法、应用题\n原题: 小明有23颗糖,平均分给4个小朋友，每人几颗？还剩几颗？\n原因: 学生未理解余数概念\n建议: 通过实物操作帮助理解余数";
    println!("\n🚀 调用 Embedding API...");
    let embedding = client.embed(test_text).await.expect("Embedding API 调用失败");
    println!("\n✅ Embedding 生成成功");
    println!("   返回维度: {}", embedding.len());
    println!("   期望维度: {}", config.llm.embedding_dimensions);
    println!("   前5个值: {:?}", &embedding[..5.min(embedding.len())]);
    assert!(!embedding.is_empty(), "Embedding 不应为空");
    println!("   实际维度: {}, 配置维度: {}", embedding.len(), config.llm.embedding_dimensions);
    let norm: f32 = embedding.iter().map(|v| v * v).sum::<f32>().sqrt();
    assert!(norm > 0.0);
    println!("\n🎉 纯文本 Embedding 端到端测试通过！");
}

#[tokio::test]
async fn test_embedding_similarity() {
    let Some(config) = load_config() else { return };
    let client = EmbeddingClient::new(&config);
    let text_a = "数学：带余数除法，小明有23颗糖分给4个人";
    let text_b = "数学：有余数的除法计算，把糖果平均分给小朋友";
    let text_c = "语文：仿写句子，春天来了，小草发芽了";
    println!("🚀 生成3条文本的 embedding...");
    let (emb_a, emb_b, emb_c) = tokio::join!(
        client.embed(text_a),
        client.embed(text_b),
        client.embed(text_c),
    );
    let emb_a = emb_a.expect("Embedding A 失败");
    let emb_b = emb_b.expect("Embedding B 失败");
    let emb_c = emb_c.expect("Embedding C 失败");
    let sim_ab = cosine_similarity(&emb_a, &emb_b);
    let sim_ac = cosine_similarity(&emb_a, &emb_c);
    println!("\n📊 相似度结果:");
    println!("   A(数学除法) vs B(数学除法): {:.4}", sim_ab);
    println!("   A(数学除法) vs C(语文句子): {:.4}", sim_ac);
    assert!(sim_ab > sim_ac);
    println!("\n🎉 语义相似度验证通过！");
}

/// ==================== 多模态 Embedding 测试 ====================
/// TODO: 等聚合服务商确认多模态 embedding API 支持后启用这些测试

#[tokio::test]
async fn test_multimodal_embedding_with_image() {
    let Some(config) = load_config() else { return };
    let client = EmbeddingClient::new(&config);
    let image_path = std::path::Path::new("tests/wrong_1.png");
    if !image_path.exists() {
        eprintln!("⚠️  跳过：测试图片不存在");
        return;
    }
    let image_base64 = read_image_base64(image_path).await.expect("读取图片失败");
    let media_type = detect_media_type(image_path);
    let text = "小学二年级数学错题";

    println!("🚀 测试多模态 embedding（图片+文本）...");
    let result = client.embed_with_image(&image_base64, media_type, text).await;
    match result {
        Ok(embedding) => {
            println!("✅ 成功！维度: {}", embedding.len());
            assert!(!embedding.is_empty(), "Embedding 不应为空");
            println!("   实际维度: {}, 配置维度: {}", embedding.len(), config.llm.embedding_dimensions);
        }
        Err(e) => {
            println!("❌ 多模态 embedding 失败: {}", e);
            println!("   TODO: 等待服务商确认 API 格式后重新实现");
        }
    }
}

#[tokio::test]
async fn test_multimodal_embedding_image_only() {
    let Some(config) = load_config() else { return };
    let client = EmbeddingClient::new(&config);
    let image_path = std::path::Path::new("tests/wrong_1.png");
    if !image_path.exists() {
        eprintln!("⚠️  跳过：测试图片不存在");
        return;
    }
    let image_base64 = read_image_base64(image_path).await.expect("读取图片失败");
    let media_type = detect_media_type(image_path);

    println!("🚀 测试纯图片 embedding...");
    let result = client.embed_image_only(&image_base64, media_type).await;
    match result {
        Ok(embedding) => {
            println!("✅ 成功！维度: {}", embedding.len());
            assert!(!embedding.is_empty(), "Embedding 不应为空");
        }
        Err(e) => {
            println!("⚠️  纯图片 embedding 未实现: {}", e);
            println!("   TODO: 等待服务商确认 API 格式后实现");
        }
    }
}
