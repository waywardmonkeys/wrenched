use std::path::{Path, PathBuf};

use accesskit::Role;
use kurbo::{Affine, Cap, Join, Line, Rect, Stroke, Vec2};
use masonry::{EventCtx, PointerEvent, Widget};
use parley::{
    Alignment, Cluster, Decoration, FontContext, FontStyle, GlyphRun, Layout,
    LayoutContext, PositionedLayoutItem, RangedBuilder, RunMetrics, StyleProperty,
};
use peniko::{BlendMode, Color, Fill, Image, ImageFormat};
use pulldown_cmark::{
    BrokenLinkCallback, Event, HeadingLevel, Options, Parser, Tag, TagEnd,
};
use smallvec::SmallVec;
use tracing::{debug, error, info, warn};
use vello::Scene;
use xilem::{
    core::{Message, MessageResult, View, ViewMarker},
    FontWeight, Pod, ViewCtx,
};

use crate::{
    layout_flow::{LayoutData, LayoutFlow},
    theme::{get_theme, Theme},
};

#[derive(Clone)]
pub enum ListMarker {
    Symbol {
        symbol: String,
        layout: Box<Layout<MarkdownBrush>>,
    },
    Numbers {
        start_number: u32,
        layouted: Vec<Layout<MarkdownBrush>>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct MarkdownBrush(Color);

impl Default for MarkdownBrush {
    fn default() -> Self {
        MarkdownBrush(Color::from_rgb8(0x00, 0x00, 0x00))
    }
}

#[derive(Clone)]
pub struct List {
    list: Vec<LayoutFlow<MarkdownContent>>,
    marker: ListMarker,
    indentation: f32,
}

#[derive(Clone)]
pub struct IndentationDecoration {}

#[derive(Clone)]
pub enum MarkdownContent {
    Indented {
        decoration: IndentationDecoration,
        flow: LayoutFlow<MarkdownContent>,
    },
    Header {
        level: HeadingLevel,
        text: String,
        markers: Vec<TextMarker>,
        text_layout: Layout<MarkdownBrush>,
    },
    List {
        list: List,
    },
    Paragraph {
        top_margin: f32,
        text: String,
        markers: Vec<TextMarker>,
        text_layout: Layout<MarkdownBrush>,
    },
    Image {
        uri: String,
        title: String,
        image: Option<Image>,
    },
    CodeBlock {
        text: String,
        text_layout: Layout<MarkdownBrush>,
    },
    HorizontalLine {
        height: f32,
    },
}

impl MarkdownContent {
    fn layout(
        &mut self,
        font_ctx: &mut FontContext,
        layout_ctx: &mut LayoutContext<MarkdownBrush>,
        width: f32,
        theme: &Theme,
    ) {
        match self {
            MarkdownContent::Paragraph {
                text,
                markers,
                top_margin: _,
                text_layout,
            } => {
                let mut builder =
                    text_to_builder(text, markers, font_ctx, layout_ctx);
                let mut layout = builder.build(&text);
                layout.break_all_lines(Some(width));
                *text_layout = layout;
            }
            MarkdownContent::Image {
                uri,
                title: _,
                image,
            } => {
                // TODO: This is a bit fishy place to load images
                if image.is_none() {
                    // TODO: Do something about unwraps
                    // Maybe show broken link image or something
                    let image_data = image::open(uri).unwrap().to_rgba8();
                    let (width, height) = image_data.dimensions();
                    *image = Some(Image::new(
                        image_data.to_vec().into(),
                        ImageFormat::Rgba8,
                        width,
                        height,
                    ));
                }
            }
            MarkdownContent::CodeBlock {
                text: _,
                text_layout: _,
            } => {}
            MarkdownContent::Indented {
                flow,
                decoration: _,
            } => {
                flow.apply_to_all(|data| {
                    data.layout(
                        font_ctx,
                        layout_ctx,
                        width - theme.markdown_indentation_decoration_width,
                        theme,
                    );
                });

                // TODO: Draw indentation decoration
            }
            MarkdownContent::List { list } => {
                let indentation: f32 = match &mut list.marker {
                    ListMarker::Symbol { symbol, layout } => {
                        let mut builder =
                            text_to_builder(symbol, &[], font_ctx, layout_ctx);
                        let mut marker_layout = builder.build(&symbol);
                        // TODO: Maybe it should get some width to prevent some stupid behaviour in some
                        // corner cases
                        marker_layout.break_all_lines(None);
                        *layout = Box::new(marker_layout);
                        layout.full_width()
                            + theme.markdown_bullet_list_indentation
                            + theme.markdown_list_after_indentation
                    }
                    ListMarker::Numbers {
                        start_number,
                        layouted,
                    } => {
                        let mut max_width: f32 = 0.0;
                        layouted.clear();
                        for k in 0..list.list.len() {
                            // Not ideal way to layout the numbered list, but works for now.
                            let mut str = (k as u32 + *start_number).to_string();
                            str.push('.');
                            let mut builder =
                                text_to_builder(&str, &[], font_ctx, layout_ctx);
                            let mut marker_layout = builder.build(&str);
                            // TODO: Maybe it should get some width to prevent some stupid behaviour in some
                            // corner cases
                            marker_layout.break_all_lines(None);
                            marker_layout.align(None, Alignment::End);
                            let width = marker_layout.full_width()
                                + theme.markdown_numbered_list_indentation
                                + theme.markdown_list_after_indentation;
                            if width > max_width {
                                max_width = width;
                            }
                            layouted.push(marker_layout);
                        }
                        max_width
                    }
                };
                list.indentation = indentation;

                for element in list.list.iter_mut() {
                    element.apply_to_all(|data| {
                        data.layout(
                            font_ctx,
                            layout_ctx,
                            width - indentation,
                            theme,
                        );
                    });
                }
            }
            MarkdownContent::HorizontalLine { height: _ } => {}
            MarkdownContent::Header {
                level,
                text,
                text_layout,
                markers,
            } => {
                let mut builder =
                    text_to_builder(text, markers, font_ctx, layout_ctx);
                let font_size = match level {
                    HeadingLevel::H1 => theme.text_size as f32 * 2.125,
                    HeadingLevel::H2 => theme.text_size as f32 * 1.875,
                    HeadingLevel::H3 => theme.text_size as f32 * 1.5,
                    HeadingLevel::H4 => theme.text_size as f32 * 1.25,
                    HeadingLevel::H5 => theme.text_size as f32 * 1.125,
                    HeadingLevel::H6 => theme.text_size as f32,
                };
                let line_height = match level {
                    // TODO: Experiment with line height to get better results???
                    HeadingLevel::H1 => 2.0,
                    HeadingLevel::H2 => 2.0,
                    HeadingLevel::H3 => 2.0,
                    HeadingLevel::H4 => 2.0,
                    HeadingLevel::H5 => 2.0,
                    HeadingLevel::H6 => 2.0,
                };
                builder.push_default(StyleProperty::FontSize(font_size));
                builder.push_default(StyleProperty::LineHeight(line_height));
                builder.push_default(StyleProperty::FontWeight(FontWeight::BOLD));
                let mut layout = builder.build(&text);
                layout.break_all_lines(Some(width));
                *text_layout = layout;
            }
        }
    }

    // TODO: Unify paint and draw call names.
    fn paint(
        &self,
        scene: &mut vello::Scene,
        mut translation: Vec2,
        source_rect: &Rect,
        theme: &Theme,
    ) {
        match self {
            MarkdownContent::Paragraph {
                top_margin: _,
                text: _,
                markers: _,
                text_layout,
            } => draw_text(scene, text_layout, translation, source_rect),
            MarkdownContent::Image {
                uri: _,
                title: _,
                image,
            } => {
                if let Some(image) = image {
                    draw_image(scene, image, translation);
                }
            }
            MarkdownContent::CodeBlock {
                text: _,
                text_layout: _,
            } => todo!(),
            MarkdownContent::Indented {
                flow,
                decoration: _,
            } => {
                let mut translation_elem = translation;
                translation_elem.x +=
                    theme.markdown_indentation_decoration_width as f64;
                draw_flow(scene, flow, translation_elem, source_rect, theme, false);
            }
            MarkdownContent::List { list } => {
                // TODO: Maybe it should get some width to prevent some stupid behaviour in some
                // corner cases
                // TODO: Maybe the LayoutFlow should have similar interface to list so it can be
                // easily used to make the list bullet point and other stuff.
                for (index, flow) in list.list.iter().enumerate() {
                    let mut translation_elem = translation;
                    translation_elem.x += list.indentation as f64;
                    draw_flow(
                        scene,
                        flow,
                        translation_elem,
                        source_rect,
                        theme,
                        false,
                    );
                    match &list.marker {
                        ListMarker::Symbol { symbol: _, layout } => {
                            let mut marker_translation = translation;
                            marker_translation.x +=
                                theme.markdown_bullet_list_indentation as f64;
                            draw_text(
                                scene,
                                layout,
                                marker_translation,
                                source_rect,
                            );
                        }
                        ListMarker::Numbers {
                            start_number: _,
                            layouted,
                        } => {
                            let mut marker_translation = translation;
                            marker_translation.x += (list.indentation
                                - layouted[index].full_width()
                                - theme.markdown_list_after_indentation)
                                as f64;
                            draw_text(
                                scene,
                                &layouted[index],
                                marker_translation,
                                source_rect,
                            );
                        }
                    }
                    translation.y += flow.height() as f64;
                }
            }
            MarkdownContent::HorizontalLine { height: _ } => todo!(),
            MarkdownContent::Header {
                level: _,
                text: _,
                text_layout,
                markers: _,
            } => {
                draw_text(scene, text_layout, translation, source_rect);
            }
        }
    }
}

impl LayoutData for MarkdownContent {
    fn height(&self) -> f32 {
        match self {
            MarkdownContent::Paragraph {
                top_margin,
                text: _,
                markers: _,
                text_layout,
            } => text_layout.height() + top_margin,
            MarkdownContent::Image {
                uri: _,
                title: _,
                image,
            } => image.as_ref().map(|i| i.height as f32).unwrap_or(0.0),
            MarkdownContent::CodeBlock {
                text: _,
                text_layout,
            } => text_layout.height(),
            MarkdownContent::Indented {
                flow,
                decoration: _,
            } => flow.height(),
            MarkdownContent::List { list } => {
                list.list.iter().map(|l| l.height()).sum()
            }
            MarkdownContent::HorizontalLine { height } => *height,
            MarkdownContent::Header {
                level: _,
                text: _,
                text_layout,
                markers: _,
            } => text_layout.height(),
        }
    }
}

#[derive(Clone)]
pub struct TextMarker {
    // TODO: Think about making it into range
    start_pos: usize,
    end_pos: usize,
    kind: MarkerKind,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum MarkerKind {
    Bold,
    Italic,
    Strikethrough,
    InlineCode,
}

fn process_image_events<'a, T: BrokenLinkCallback<'a>>(
    events: &mut Parser<'a, T>,
) -> String {
    let mut text = String::new();
    for event in events {
        match event {
            Event::Text(cow_str) => text = cow_str.to_string(),
            Event::End(TagEnd::Image) => return text,
            e => {
                error!("Image tag parsing expects only Text event but {e:?} was received")
            }
        }
    }
    error!("Image tag parsing expects Image End tag and none was received");
    String::new()
}

struct MarkeerState {
    bold_start: usize,
    italic_start: usize,
    strikethrough_start: usize,
    markers: Vec<TextMarker>,
}

impl MarkeerState {
    fn new() -> Self {
        Self {
            bold_start: 0,
            italic_start: 0,
            strikethrough_start: 0,
            markers: Vec::new(),
        }
    }
}

fn process_marker(
    event: &Event,
    marker_state: &mut MarkeerState,
    text_end: usize,
) -> bool {
    match event {
        Event::Start(Tag::Strong) => {
            marker_state.bold_start = text_end;
            true
        }
        Event::Start(Tag::Emphasis) => {
            marker_state.italic_start = text_end;
            true
        }
        Event::Start(Tag::Strikethrough) => {
            marker_state.strikethrough_start = text_end;
            true
        }
        Event::End(TagEnd::Strong) => {
            marker_state.markers.push(TextMarker {
                start_pos: marker_state.bold_start,
                end_pos: text_end,
                kind: MarkerKind::Bold,
            });
            true
        }
        Event::End(TagEnd::Emphasis) => {
            marker_state.markers.push(TextMarker {
                start_pos: marker_state.strikethrough_start,
                end_pos: text_end,
                kind: MarkerKind::Italic,
            });
            true
        }
        Event::End(TagEnd::Strikethrough) => {
            marker_state.markers.push(TextMarker {
                start_pos: marker_state.strikethrough_start,
                end_pos: text_end,
                kind: MarkerKind::Strikethrough,
            });
            true
        }
        _ => false,
    }
}

fn process_header_events<'a, T: BrokenLinkCallback<'a>>(
    events: &mut Parser<'a, T>,
    header_level: &HeadingLevel,
) -> MarkdownContent {
    let mut text = String::new();
    let mut marker_state = MarkeerState::new();
    for event in events {
        if process_marker(&event, &mut marker_state, text.len()) {
            continue;
        }
        match event {
            Event::Text(cow_str) => text.push_str(&cow_str),
            Event::End(TagEnd::Heading(_)) => {
                return MarkdownContent::Header {
                    level: *header_level,
                    text,
                    markers: marker_state.markers,
                    text_layout: Layout::new(),
                }
            }
            e => {
                error!("Header tag parsing expects only some event but {e:?} was received")
            }
        }
    }
    panic!("Header tag parsing expects Heading end tag and none was received");
}

fn process_list_events<'a, T: BrokenLinkCallback<'a>>(
    events: &mut Parser<'a, T>,
) -> Vec<LayoutFlow<MarkdownContent>> {
    let mut list_elements = Vec::new();

    while let Some(event) = events.next() {
        println!("Event: {event:?}");
        if let Event::Start(Tag::Item) = event {
            list_elements
                .push(process_events(events, Some(Event::End(TagEnd::Item))));
        } else if let Event::End(TagEnd::List(_)) = event {
            break;
        } else {
            panic!("List tag parsing expects List end tag; received {event:?}");
        }
    }
    list_elements
}

fn process_events<'a, T: BrokenLinkCallback<'a>>(
    events: &mut Parser<'a, T>,
    untill: Option<Event>,
) -> LayoutFlow<MarkdownContent> {
    let mut res = LayoutFlow::new();

    let mut text = String::new();
    let mut marker_state = MarkeerState::new();

    // TODO: Make sure the firsts element margin is 0.0.
    while let Some(event) = events.next() {
        println!("Event: {event:?}");
        if let Some(event_) = &untill {
            if &event == event_ {
                break;
            }
        }
        if process_marker(&event, &mut marker_state, text.len()) {
            continue;
        }
        match event {
            Event::Start(tag) => match &tag {
                Tag::Image {
                    link_type: _,
                    dest_url,
                    title,
                    id: _,
                } => {
                    // TODO: Use the text...
                    // TODO: Should the image be loaded here???
                    // TODO: Maybe images should be done as markers instead and I
                    // should just collect images into some `HashMap`.
                    let _some_text = process_image_events(events);
                    res.push(MarkdownContent::Image {
                        uri: dest_url.to_string(),
                        title: title.to_string(),
                        image: None,
                    })
                }
                Tag::CodeBlock(_kind) => { // TODO: Add code block
                }
                Tag::Table(_alignments) => {
                    warn!("Markdown tables not supported")
                }
                Tag::Paragraph => {}
                Tag::Heading {
                    level,
                    id: _,
                    classes: _,
                    attrs: _,
                } => res.push(process_header_events(events, level)),
                Tag::BlockQuote(block_quote_kind) => {
                    let flow = process_events(
                        events,
                        Some(Event::End(TagEnd::BlockQuote(*block_quote_kind))),
                    );
                    res.push(MarkdownContent::Indented {
                        decoration: IndentationDecoration {},
                        flow,
                    });
                }
                Tag::HtmlBlock => todo!(),
                Tag::List(list_marker) => {
                    let list = process_list_events(events);
                    // TODO: Think about the markers. There should be a better way to set them up
                    let marker = if let Some(list_marker) = list_marker {
                        ListMarker::Numbers {
                            start_number: *list_marker as u32,
                            layouted: Vec::new(),
                        }
                    } else {
                        ListMarker::Symbol {
                            symbol: "•".to_string(),
                            layout: Box::new(Layout::new()),
                        }
                    };
                    res.push(MarkdownContent::List {
                        list: List {
                            marker,
                            list,
                            indentation: 0.0,
                        },
                    });
                }
                Tag::FootnoteDefinition(_cow_str) => todo!(),
                Tag::DefinitionList => {
                    warn!("DefinitionList in markdown is not supported!")
                }
                Tag::DefinitionListTitle => {
                    warn!("DefinitionList in markdown is not supported!")
                }
                Tag::DefinitionListDefinition => {
                    warn!("DefinitionList in markdown is not supported!")
                }
                Tag::TableHead => todo!(),
                Tag::TableRow => todo!(),
                Tag::TableCell => todo!(),
                Tag::Link {
                    link_type: _,
                    dest_url: _,
                    title: _,
                    id: _,
                } => todo!(),
                Tag::MetadataBlock(_metadata_block_kind) => {
                    warn!("MetadataBlock in markdown are not supported")
                }
                _ => {}
            },
            Event::End(end_tag) => {
                match end_tag {
                    TagEnd::Paragraph => {
                        // TODO: Work on the top_margin
                        if !text.trim().is_empty() {
                            res.push(MarkdownContent::Paragraph {
                                top_margin: 10.0,
                                text: text.clone(),
                                markers: marker_state.markers.clone(),
                                text_layout: Layout::new(),
                            });
                            text.clear();
                            marker_state.markers.clear();
                        }
                    }
                    TagEnd::CodeBlock => todo!(),
                    TagEnd::HtmlBlock => todo!(),
                    TagEnd::FootnoteDefinition => todo!(),
                    TagEnd::Table => todo!(),
                    TagEnd::TableHead => todo!(),
                    TagEnd::TableRow => todo!(),
                    TagEnd::TableCell => todo!(),
                    TagEnd::Link => todo!(),
                    e => {
                        warn!("Markdown parsing unprocessed end tag: {e:?}");
                    }
                }
            }
            Event::Text(text_bit) => {
                // TODO: Ignore text in some cases???
                text.push_str(&text_bit);
            }
            Event::Code(text_bit) => {
                // TODO: Maybe it should be a text_manager with both text and markers.
                marker_state.markers.push(TextMarker {
                    start_pos: text.len(),
                    end_pos: text.len() + text_bit.len(),
                    kind: MarkerKind::InlineCode,
                });
                text.push_str(&text_bit);
            }
            Event::Html(text_bit) => {
                // TODO: This looks a bit fishy
                marker_state.markers.push(TextMarker {
                    start_pos: text.len(),
                    end_pos: text.len() + text_bit.len(),
                    kind: MarkerKind::InlineCode,
                });
                text.push_str(&text_bit);
            }
            Event::HardBreak => {
                text.push('\n');
            }
            Event::SoftBreak => {
                text.push(' ');
            }
            Event::Rule => {
                // This adds random value. It will be recalculated anyway.
                // TODO: Maybe it there should be additional step which adds
                // these heights based on the theme???
                res.push(MarkdownContent::HorizontalLine { height: 0.0 })
            }
            Event::FootnoteReference(_text) => {
                warn!("FootnoteReference in markdown is not supported!")
            }
            Event::TaskListMarker(_marker) => {
                warn!("TaskListMarker in markdown is not supported!")
            }
            Event::InlineHtml(_) => {
                warn!("InlineHtml in markdown is not supported!")
            }
            Event::InlineMath(_) => {
                warn!("InlineMath in markdown is not supported!")
            }
            Event::DisplayMath(_) => {
                warn!("DisplayMath in markdown is not supported!")
            }
        }
    }

    if !text.is_empty() {
        res.push(MarkdownContent::Paragraph {
            // TODO: Make nice offset
            // TODO: This should be in theme as well
            // TODO: It should be relative to the font size
            top_margin: 12.0,
            text,
            markers: marker_state.markers,
            text_layout: Layout::new(),
        });
    }

    res
}

fn parse_markdown(text: &str) -> LayoutFlow<MarkdownContent> {
    let mut parser = Parser::new_ext(
        text,
        //Options::ENABLE_TABLES
        //| Options::ENABLE_FOOTNOTES
        //| Options::ENABLE_STRIKETHROUGH
        Options::ENABLE_STRIKETHROUGH, //| Options::ENABLE_TASKLISTS
                                       //| Options::ENABLE_HEADING_ATTRIBUTES,
    );

    process_events(&mut parser, None)
}

fn feed_marker_to_builder<'a>(
    builder: &'a mut RangedBuilder<MarkdownBrush>,
    text_marker: &TextMarker,
    theme: &'a Theme,
) {
    let rang = text_marker.start_pos..text_marker.end_pos;
    match text_marker.kind {
        MarkerKind::Bold => {
            builder.push(StyleProperty::FontWeight(FontWeight::BOLD), rang)
        }
        MarkerKind::Italic => {
            builder.push(StyleProperty::FontStyle(FontStyle::Italic), rang)
        }
        MarkerKind::Strikethrough => {
            builder.push(StyleProperty::Strikethrough(true), rang)
        }
        MarkerKind::InlineCode => {
            builder.push(
                StyleProperty::FontStack(theme.monospace_font_stack.clone()),
                rang.clone(),
            );
            builder.push(
                StyleProperty::Brush(MarkdownBrush(theme.monospace_text_color)),
                rang,
            );
        }
    }
}

fn text_to_builder<'a>(
    text: &'a str,
    markers: &[TextMarker],
    font_ctx: &'a mut FontContext,
    layout_ctx: &'a mut LayoutContext<MarkdownBrush>,
) -> RangedBuilder<'a, MarkdownBrush> {
    let theme = get_theme();

    let mut builder: RangedBuilder<'_, MarkdownBrush> =
        layout_ctx.ranged_builder(font_ctx, text, theme.scale);
    builder.push_default(StyleProperty::Brush(MarkdownBrush(theme.text_color)));
    builder.push_default(StyleProperty::FontSize(theme.text_size as f32));
    builder.push_default(StyleProperty::FontStack(theme.font_stack.clone()));
    builder.push_default(StyleProperty::FontWeight(FontWeight::NORMAL));
    builder.push_default(StyleProperty::FontStyle(FontStyle::Normal));
    builder.push_default(StyleProperty::LineHeight(1.0));
    for marker in markers.iter() {
        feed_marker_to_builder(&mut builder, marker, &theme);
    }
    builder
}

pub struct MarkdowWidget {
    markdown_layout: LayoutFlow<MarkdownContent>,
    layout_ctx: LayoutContext<MarkdownBrush>,
    max_advance: f64,
    dirty: bool,
    scroll: Vec2,
}

impl MarkdowWidget {
    pub fn new<P: AsRef<Path>>(markdown_file: P) -> Self {
        // TODO: Ehm... unwraps...
        let content: String =
            String::from_utf8(std::fs::read(&markdown_file).unwrap()).unwrap();
        let markdown_layout = parse_markdown(&content);
        Self {
            markdown_layout,
            dirty: true,
            layout_ctx: LayoutContext::new(),
            max_advance: 0.0,
            scroll: Vec2::new(0.0, 0.0),
        }
    }
}
fn draw_underline(
    scene: &mut Scene,
    underline: &Decoration<MarkdownBrush>,
    glyph_run: &GlyphRun<'_, MarkdownBrush>,
    run_metrics: &RunMetrics,
    transform: &Affine,
) {
    let offset = underline.offset.unwrap_or(run_metrics.underline_offset);
    let stroke_size = underline.size.unwrap_or(run_metrics.underline_size);
    let y1 = glyph_run.baseline() - offset - (stroke_size / 2.0);
    let x1 = glyph_run.offset();
    let x2 = x1 + glyph_run.advance();
    let underline_shape = Line::new((x1, y1), (x2, y1));

    let stroke = Stroke {
        width: stroke_size as f64,
        join: Join::Bevel,
        miter_limit: 4.0,
        start_cap: Cap::Butt,
        end_cap: Cap::Butt,
        dash_pattern: Default::default(),
        dash_offset: 0.0,
    };

    scene.stroke(
        &stroke,
        *transform,
        underline.brush.0,
        Some(Affine::IDENTITY),
        &underline_shape,
    );
}

fn draw_strikethrough(
    scene: &mut Scene,
    strikethrough: &Decoration<MarkdownBrush>,
    glyph_run: &GlyphRun<'_, MarkdownBrush>,
    run_metrics: &RunMetrics,
    transform: &Affine,
) {
    let offset = strikethrough
        .offset
        .unwrap_or(run_metrics.strikethrough_offset);
    let size = strikethrough.size.unwrap_or(run_metrics.strikethrough_size);
    // FIXME: This offset looks fishy... I think I should add it instead.
    let y1 = glyph_run.baseline() - offset - (size / 2.0);
    let x1 = glyph_run.offset();
    let x2 = x1 + glyph_run.advance();
    let strikethrough_shape = Line::new((x1, y1), (x2, y1));

    let stroke = Stroke {
        width: size as f64,
        join: Join::Bevel,
        miter_limit: 4.0,
        start_cap: Cap::Butt,
        end_cap: Cap::Butt,
        dash_pattern: Default::default(),
        dash_offset: 0.0,
    };

    scene.stroke(
        &stroke,
        *transform,
        strikethrough.brush.0,
        Some(Affine::IDENTITY),
        &strikethrough_shape,
    );
}

fn draw_text(
    scene: &mut Scene,
    layout: &Layout<MarkdownBrush>,
    translation: Vec2,
    source_rect: &Rect,
) {
    let transform: Affine = Affine::translate(translation);
    let mut top_line_index = if let Some((cluster, _)) =
        Cluster::from_point(layout, 0.0, source_rect.y0 as f32)
    {
        cluster.path().line_index()
    } else {
        0
    };
    while let Some(line) = layout.get(top_line_index) {
        let line_metrics = line.metrics();
        if line_metrics.min_coord > source_rect.y1 as f32 {
            break;
        }
        for item in line.items() {
            let PositionedLayoutItem::GlyphRun(glyph_run) = item else {
                continue;
            };
            let style = glyph_run.style();
            let text_color = &style.brush;

            let run = glyph_run.run();
            // TODO: This needs to be some kind of a flow layout.
            let font = run.font();
            let font_size = run.font_size();
            let synthesis = run.synthesis();
            let glyph_xform = synthesis
                .skew()
                .map(|angle| Affine::skew(angle.to_radians().tan() as f64, 0.0));
            let coords = run.normalized_coords();
            scene
                .draw_glyphs(font)
                .brush(text_color.0)
                .hint(true)
                .transform(transform)
                .glyph_transform(glyph_xform)
                .font_size(font_size)
                .normalized_coords(&coords)
                .draw(
                    Fill::NonZero,
                    glyph_run.positioned_glyphs().map(|glyph| vello::Glyph {
                        id: glyph.id as _,
                        x: glyph.x,
                        y: glyph.y,
                    }),
                );

            let run_metrics = run.metrics();
            if let Some(underline) = &style.underline {
                draw_underline(
                    scene,
                    underline,
                    &glyph_run,
                    run_metrics,
                    &transform,
                );
            }

            if let Some(strikethrough) = &style.strikethrough {
                draw_strikethrough(
                    scene,
                    strikethrough,
                    &glyph_run,
                    run_metrics,
                    &transform,
                );
            }
        }
        top_line_index += 1;
    }
}

fn draw_image(scene: &mut Scene, image: &Image, translation: Vec2) {
    let transform: Affine = Affine::translate(translation);
    scene.draw_image(image, transform);
}

fn draw_flow(
    scene: &mut Scene,
    flow: &LayoutFlow<MarkdownContent>,
    source_translation: Vec2,
    source_rect: &Rect,
    theme: &Theme,
    apply_scroll: bool,
) {
    let visible_parts = flow.get_visible_parts(
        source_rect.y0 as f32,
        (source_rect.y1 - source_rect.y0) as f32,
    );

    let offset = if apply_scroll { source_rect.y0 } else { 0.0 };
    for visible_part in visible_parts {
        let translation =
            source_translation + Vec2::new(0.0, visible_part.offset as f64 - offset);
        visible_part.get_source_rect(source_rect);
        let sub_source_rect = visible_part.get_source_rect(source_rect);
        visible_part
            .data
            .paint(scene, translation, &sub_source_rect, theme);
    }
}

impl Widget for MarkdowWidget {
    fn on_pointer_event(&mut self, ctx: &mut EventCtx, event: &PointerEvent) {
        println!("event: {event:?} >>> ctx: {}", ctx.size());
        if let PointerEvent::MouseWheel(delta, _) = event {
            const SCROLLING_SPEED: f64 = 3.0;
            let delta =
                Vec2::new(delta.x * -SCROLLING_SPEED, delta.y * -SCROLLING_SPEED);
            self.scroll += delta;
            let size = ctx.size();
            let baseline = ctx.baseline_offset();
            self.scroll.x = self.scroll.x.max(0.0);
            self.scroll.y = self.scroll.y.max(0.0);
            // TODO: Get corrent view port width so the horizontal scroll is
            // possible.
            self.scroll.x = self.scroll.x.min(0.0);
            self.scroll.y = self
                .scroll
                .y
                .min(self.markdown_layout.height() as f64 - size.height + baseline);
            info!("scrolling new scroll: {} , self.markdown_layout.height() {}, ctx.size() {}", self.scroll, self.markdown_layout.height(), ctx.size());
            if let Some(bla) = self.markdown_layout.flow.last() {
                info!("bla.offset: {}", bla.offset);
            }
            ctx.request_paint_only();
            ctx.set_handled();
        }
    }

    fn register_children(&mut self, _ctx: &mut masonry::RegisterCtx) {}

    fn compose(&mut self, ctx: &mut masonry::ComposeCtx) {
        info!("compose called: size: {}, baseline_offset: {}, window_origin: {}, layout_rect: {}", ctx.size(), ctx.baseline_offset(), ctx.window_origin(), ctx.layout_rect());
    }

    fn layout(
        &mut self,
        ctx: &mut masonry::LayoutCtx,
        bc: &masonry::BoxConstraints,
    ) -> kurbo::Size {
        debug!("cool layout");
        let size = bc.max();
        let theme = &get_theme();
        // TODO: Think about putting the context into the theme??? Or somewhere else???
        let (font_ctx, _layout_ctx) = ctx.text_contexts();
        if self.dirty || self.max_advance != size.width {
            self.markdown_layout.apply_to_all(|data| {
                data.layout(
                    font_ctx,
                    &mut self.layout_ctx,
                    size.width as f32,
                    theme,
                );
            });
        }

        self.max_advance = size.width;
        self.dirty = false;
        info!("size: {}", size);
        size
    }

    fn paint(&mut self, ctx: &mut masonry::PaintCtx, scene: &mut vello::Scene) {
        scene.push_layer(
            BlendMode::default(),
            1.,
            Affine::IDENTITY,
            &ctx.size().to_rect(),
        );
        // TODO: Make scroll work
        let source_rect =
            Rect::new(0.0, self.scroll.y, 0.0, self.scroll.y + ctx.size().height);
        let theme = &get_theme();
        draw_flow(
            scene,
            &self.markdown_layout,
            Vec2::new(0.0, 0.0),
            &source_rect,
            theme,
            true,
        );
        scene.pop_layer();
    }

    fn accessibility_role(&self) -> accesskit::Role {
        Role::Document
    }

    fn accessibility(
        &mut self,
        _ctx: &mut masonry::AccessCtx,
        _node: &mut accesskit::Node,
    ) {
    }

    fn children_ids(&self) -> SmallVec<[masonry::WidgetId; 16]> {
        SmallVec::new()
    }
}

///// Highlight the text in a richtext builder like it was a markdown codeblock
//pub fn highlight_as_code(
//    attr_list: &mut AttrsList,
//    default_attrs: Attrs,
//    language: Option<LapceLanguage>,
//    text: &str,
//    start_offset: usize,
//    config: &LapceConfig,
//) {
//    let syntax = language.map(Syntax::from_language);
//
//    let styles = syntax
//        .map(|mut syntax| {
//            syntax.parse(0, Rope::from(text), None);
//            syntax.styles
//        })
//        .unwrap_or(None);
//
//    if let Some(styles) = styles {
//        for (range, style) in styles.iter() {
//            if let Some(color) = style
//                .fg_color
//                .as_ref()
//                .and_then(|fg| config.style_color(fg))
//            {
//                attr_list.add_span(
//                    start_offset + range.start..start_offset + range.end,
//                    default_attrs.color(color),
//                );
//            }
//        }
//    }
//}

pub struct MarkdownView {
    path: PathBuf,
}

pub fn markdown_view(path: PathBuf) -> MarkdownView {
    MarkdownView { path }
}

impl ViewMarker for MarkdownView {}
impl<State, Action> View<State, Action, ViewCtx> for MarkdownView
where
    State: 'static,
    Action: 'static,
{
    type Element = Pod<MarkdowWidget>;

    type ViewState = ();

    fn build(&self, ctx: &mut ViewCtx) -> (Self::Element, Self::ViewState) {
        debug!("CodeView::build");
        ctx.with_leaf_action_widget(|ctx| {
            ctx.new_pod(MarkdowWidget::new(&self.path))
        })
    }

    fn rebuild(
        &self,
        _prev: &Self,
        _view_state: &mut Self::ViewState,
        _ctx: &mut ViewCtx,
        _element: xilem::core::Mut<Self::Element>,
    ) {
        debug!("CodeView::rebuild");
    }

    fn teardown(
        &self,
        _view_state: &mut Self::ViewState,
        ctx: &mut ViewCtx,
        element: xilem::core::Mut<Self::Element>,
    ) {
        debug!("CodeView::teardown");
        ctx.teardown_leaf(element);
    }

    fn message(
        &self,
        _view_state: &mut Self::ViewState,
        _id_path: &[xilem::core::ViewId],
        message: Box<dyn Message>,
        _app_state: &mut State,
    ) -> xilem::core::MessageResult<Action, Box<dyn Message>> {
        debug!("CodeView::message");
        match message.downcast::<masonry::Action>() {
            Ok(action) => {
                tracing::error!(
                    "Wrong action type in CodeView::message: {action:?}"
                );
                MessageResult::Stale(action)
            }
            Err(message) => {
                tracing::error!(
                    "Wrong message type in Button::message: {message:?}"
                );
                MessageResult::Stale(message)
            }
        }
    }
}
