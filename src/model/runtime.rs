use super::MessageContent;

const RUNNING_MESSAGE: &str = "The task is now running in the background.";

pub(crate) fn background_task_started_reminder(task_id: &str, name: &str) -> String {
    wrap_runtime_reminder(&[render_background_task_block(
        task_id,
        name,
        None,
        RUNNING_MESSAGE,
    )])
}

pub(crate) fn render_background_task_content(content: &[MessageContent]) -> Option<String> {
    if content.is_empty() {
        return None;
    }
    let blocks = content
        .iter()
        .map(|entry| match entry {
            MessageContent::BackgroundTask {
                task_id,
                name,
                status,
                content,
                ..
            } => Some(render_background_task_block(
                task_id,
                name,
                status.as_deref(),
                content,
            )),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()?;
    Some(wrap_runtime_reminder(&blocks))
}

pub(crate) fn render_background_task_block(
    task_id: &str,
    name: &str,
    status: Option<&str>,
    content: &str,
) -> String {
    let task_id = escape_xml_attribute(task_id);
    let name = escape_xml_attribute(name);
    let status = status
        .map(escape_xml_attribute)
        .map(|status| format!(" status=\"{status}\""))
        .unwrap_or_default();
    format!(
        "<background_task task_id=\"{task_id}\" name=\"{name}\"{status}>\n{content}\n</background_task>"
    )
}

fn wrap_runtime_reminder(blocks: &[String]) -> String {
    format!(
        "<runtime-reminder>\n{}\n</runtime-reminder>",
        blocks.join("\n\n")
    )
}

fn escape_xml_attribute(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifact::ResultMetadata;

    #[test]
    fn started_notice_is_statusless_and_escapes_the_model_supplied_name() {
        assert_eq!(
            background_task_started_reminder("t1", "review <auth> & \"tests\""),
            "<runtime-reminder>\n<background_task task_id=\"t1\" name=\"review &lt;auth&gt; &amp; &quot;tests&quot;\">\nThe task is now running in the background.\n</background_task>\n</runtime-reminder>"
        );
    }

    #[test]
    fn terminal_notices_share_one_runtime_reminder() {
        let content = vec![
            MessageContent::BackgroundTask {
                task_id: "t1".to_owned(),
                name: "tests".to_owned(),
                status: Some("completed".to_owned()),
                content: ".pico/runs/run/artifacts/t1.txt".to_owned(),
                metadata: ResultMetadata::empty(),
            },
            MessageContent::BackgroundTask {
                task_id: "t2".to_owned(),
                name: "review".to_owned(),
                status: Some("failed".to_owned()),
                content: ".pico/runs/run/artifacts/t2.txt".to_owned(),
                metadata: ResultMetadata::empty(),
            },
        ];

        let rendered = render_background_task_content(&content).unwrap();
        assert_eq!(rendered.matches("<runtime-reminder>").count(), 1);
        assert_eq!(rendered.matches("<background_task ").count(), 2);
        assert!(rendered.contains("status=\"completed\""));
        assert!(rendered.contains("status=\"failed\""));
    }
}
