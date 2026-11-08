//! Snapshot tests for all Formatter convenience methods and Renderable components.
//!
//! Each method/component is tested in plain mode (for readable, stable snapshots)
//! and with narrow terminal width (40 cols) to verify edge-case rendering.
//! Theme parity tests verify all render under all 3 theme modes without panics.

#[cfg(test)]
mod tests {
    use crate::ui::components::test_helpers::TestFormatter;
    use crate::ui::components::{
        Header, KeyValue, List, ListItem, Renderable, StatusGroup, StatusLine, Table,
    };
    use crate::ui::theme::ThemeMode;

    // ========================================================================
    // heading()
    // ========================================================================




    // ========================================================================
    // subheading()
    // ========================================================================



    // ========================================================================
    // status()
    // ========================================================================




    // ========================================================================
    // success()
    // ========================================================================



    // ========================================================================
    // warning()
    // ========================================================================



    // ========================================================================
    // error()
    // ========================================================================



    // ========================================================================
    // info()
    // ========================================================================



    // ========================================================================
    // field()
    // ========================================================================




    // ========================================================================
    // field_accent()
    // ========================================================================



    // ========================================================================
    // bullet()
    // ========================================================================




    // ========================================================================
    // separator()
    // ========================================================================



    // ========================================================================
    // indent_block()
    // ========================================================================




    // ========================================================================
    // key_hint()
    // ========================================================================




    // ========================================================================
    // progress()
    // ========================================================================






    // ========================================================================
    // Combined output (simulating a command's full output)
    // ========================================================================





    // ========================================================================
    // Edge cases
    // ========================================================================





    // ========================================================================
    // Header component
    // ========================================================================







    // ========================================================================
    // StatusLine component
    // ========================================================================







    // ========================================================================
    // KeyValue component
    // ========================================================================






    // ========================================================================
    // List component
    // ========================================================================







    // ========================================================================
    // Table component
    // ========================================================================







    // ========================================================================
    // Combined component output
    // ========================================================================



    // ========================================================================
    // Theme parity tests (including components)
    // ========================================================================

    #[test]
    fn all_components_render_dark_theme() {
        let mut tf = TestFormatter::dark();
        render_all_components(&mut tf);
        let output = tf.output_plain();
        assert!(!output.is_empty());
        assert!(output.contains("Cassy Doctor"));
        assert!(output.contains("cas-1234"));
    }

    #[test]
    fn all_components_render_light_theme() {
        let mut tf = TestFormatter::light();
        render_all_components(&mut tf);
        let output = tf.output_plain();
        assert!(!output.is_empty());
        assert!(output.contains("Cassy Doctor"));
    }

    #[test]
    fn all_components_render_high_contrast_theme() {
        let mut tf = TestFormatter::high_contrast();
        render_all_components(&mut tf);
        let output = tf.output_plain();
        assert!(!output.is_empty());
        assert!(output.contains("Cassy Doctor"));
    }

    #[test]
    fn component_theme_parity_all_modes() {
        for mode in [ThemeMode::Dark, ThemeMode::Light, ThemeMode::HighContrast] {
            let mut tf = TestFormatter::with_theme(mode, 80);
            render_all_components(&mut tf);
            let plain = tf.output_plain();
            assert!(
                !plain.is_empty(),
                "Theme {mode:?} produced empty component output"
            );
            assert!(
                plain.contains("Cassy Doctor"),
                "Theme {mode:?} missing Header"
            );
            assert!(
                plain.contains("Store connected"),
                "Theme {mode:?} missing StatusLine"
            );
            assert!(
                plain.contains("cas-1234"),
                "Theme {mode:?} missing KeyValue"
            );
            assert!(plain.contains("First item"), "Theme {mode:?} missing List");
            assert!(plain.contains("ID"), "Theme {mode:?} missing Table header");
        }
    }

    /// Render every component type for theme parity testing.
    fn render_all_components(tf: &mut TestFormatter) {
        let mut fmt = tf.fmt();
        Header::h1("Cassy Doctor").render(&mut fmt).unwrap();
        Header::h2("Section").render(&mut fmt).unwrap();
        Header::h3("Subsection").render(&mut fmt).unwrap();
        StatusLine::success("Store connected")
            .render(&mut fmt)
            .unwrap();
        StatusLine::error("Auth failed").render(&mut fmt).unwrap();
        StatusLine::warning("Config stale")
            .render(&mut fmt)
            .unwrap();
        StatusLine::info("3 tasks").render(&mut fmt).unwrap();
        KeyValue::new()
            .add("ID", "cas-1234")
            .add("Status", "open")
            .render(&mut fmt)
            .unwrap();
        List::bullet(vec![
            ListItem::new("First item"),
            ListItem::new("Second item"),
        ])
        .render(&mut fmt)
        .unwrap();
        Table::new()
            .columns(&["ID", "Name"])
            .rows(vec![vec!["1", "test"]])
            .render(&mut fmt)
            .unwrap();
    }

    // ========================================================================
    // Theme parity tests (Formatter methods)
    // ========================================================================

    #[test]
    fn all_methods_render_dark_theme() {
        let mut tf = TestFormatter::dark();
        render_all_methods(&mut tf);
        let output = tf.output_plain();
        assert!(!output.is_empty());
        assert!(output.contains("Heading"));
        assert!(output.contains("Subheading"));
    }

    #[test]
    fn all_methods_render_light_theme() {
        let mut tf = TestFormatter::light();
        render_all_methods(&mut tf);
        let output = tf.output_plain();
        assert!(!output.is_empty());
        assert!(output.contains("Heading"));
        assert!(output.contains("Subheading"));
    }

    #[test]
    fn all_methods_render_high_contrast_theme() {
        let mut tf = TestFormatter::high_contrast();
        render_all_methods(&mut tf);
        let output = tf.output_plain();
        assert!(!output.is_empty());
        assert!(output.contains("Heading"));
        assert!(output.contains("Subheading"));
    }

    #[test]
    fn theme_parity_all_modes() {
        for mode in [ThemeMode::Dark, ThemeMode::Light, ThemeMode::HighContrast] {
            let mut tf = TestFormatter::with_theme(mode, 80);
            render_all_methods(&mut tf);
            let plain = tf.output_plain();
            assert!(!plain.is_empty(), "Theme {mode:?} produced empty output");
            assert!(plain.contains("Heading"), "Theme {mode:?} missing heading");
            assert!(
                plain.contains("success message"),
                "Theme {mode:?} missing success"
            );
            assert!(
                plain.contains("error message"),
                "Theme {mode:?} missing error"
            );
            assert!(plain.contains("50%"), "Theme {mode:?} missing progress");
        }
    }




    /// Render every Formatter convenience method for parity testing.
    fn render_all_methods(tf: &mut TestFormatter) {
        let mut fmt = tf.fmt();
        fmt.heading("Heading").unwrap();
        fmt.subheading("Subheading").unwrap();
        fmt.status("Label", "Value").unwrap();
        fmt.success("success message").unwrap();
        fmt.warning("warning message").unwrap();
        fmt.error("error message").unwrap();
        fmt.info("info message").unwrap();
        fmt.field("Key", "value").unwrap();
        fmt.field_accent("Accent", "highlighted").unwrap();
        fmt.bullet("bullet item").unwrap();
        fmt.separator().unwrap();
        fmt.indent_block("indented\nblock").unwrap();
        fmt.key_hint("q", "Quit").unwrap();
        fmt.progress(5, 10).unwrap();
    }
}
