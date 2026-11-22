use submerge_core::{demo_timeline, fmt, Timeline};

fn print_report(tl: &Timeline) {
    println!(
        "视频时长: {} | 统一时间轴: 纳秒 | 空档阈值: {}",
        fmt::ns_to_timecode(tl.video_duration_ns),
        fmt::ns_to_duration(tl.gap_threshold_ns),
    );
    println!();

    let reasons = tl.reasons_by_event();
    println!(
        "{:<4} {:<10} {:<14} {:<36} {:<26} 冲突原因",
        "ID", "语言", "文本", "原始时间", "换算时间(统一)"
    );
    println!("{}", "-".repeat(128));

    let mut events: Vec<_> = tl.events().to_vec();
    events.sort_by_key(|e| (e.unified_start_ns(), e.id));
    for e in &events {
        let reason = reasons
            .get(&e.id)
            .map(|r| r.join("; "))
            .unwrap_or_else(|| "无".to_string());
        let text: String = e.text.chars().take(6).collect();
        println!(
            "{:<4} {:<10} {:<14} {:<36} {:<26} {}",
            format!("#{}", e.id),
            e.language.to_string(),
            text,
            e.original_timing(),
            e.unified_timing(),
            reason,
        );
    }

    println!();
    println!("冲突汇总:");
    let conflicts = tl.detect_conflicts();
    if conflicts.is_empty() {
        println!("  无冲突");
    }
    for c in &conflicts {
        println!("  {}", c);
    }
}

fn demo_reorder(tl: &mut Timeline) {
    println!();
    println!("局部重排: 交换 zh#2 与 zh#3 (selection=[2,3], new_order=[1,0])");
    match tl.reorder_range(&[2, 3], &[1, 0]) {
        Ok(ids) => println!("重排后顺序: {:?}\n", ids),
        Err(e) => {
            eprintln!("重排失败: {}", e);
            return;
        }
    }
    print_report(tl);
}

fn main() {
    let mut tl = demo_timeline();
    println!("=== 合并前 ===");
    print_report(&tl);
    println!();
    println!("=== 局部重排后 ===");
    demo_reorder(&mut tl);
}
