//! The Layers panel: a popover listing the layers with a kind picker and the options that
//! matter most for each kind. Built from the app's own controls (ghost buttons, dropdown
//! menus, the toggle square the layer list always had), so it reads like the rest of Theta.

use crate::classify::Method;
use crate::panel::{MapPanel, Theme, floating, muted_text};
use crate::ramp::PaletteId;
use crate::style::{
    BinValue, ColorBy, ColorScheme, ColorSpec, LayerKind, LayerStyle, RadiusBy, Simplify, TilingMode,
    tiling_enabled,
};
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{Icon, Selectable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, ElementId, InteractiveElement as _, IntoElement, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, div, hsla, px,
};

/// Width of the popover.
const W: f32 = 300.0;

type Pick = dyn Fn(&mut MapPanel, usize, &mut Context<MapPanel>);

fn section(t: &Theme, title: &str) -> gpui_kit::Div {
    v_flex().gap_1().pt_1().child(div().h(px(1.)).bg(t.border)).child(muted_text(t, title.to_string()))
}

fn row(t: &Theme, label: &str, control: impl IntoElement) -> gpui_kit::Div {
    h_flex()
        .h(px(26.))
        .gap_2()
        .items_center()
        .child(div().w(px(84.)).flex_shrink_0().child(muted_text(t, label.to_string())))
        .child(div().flex_1().min_w_0().flex().justify_end().child(control))
}

impl MapPanel {
    /// Apply `f` to the style of the layer being edited.
    pub(crate) fn edit(&mut self, f: impl FnOnce(&mut LayerStyle), cx: &mut Context<Self>) {
        let layer = self.sel_layer;
        self.core.borrow_mut().update_style(layer, f);
        cx.notify();
    }

    fn dropdown(
        &self,
        id: impl Into<ElementId>,
        current: String,
        items: Vec<String>,
        checked: Option<usize>,
        apply: impl Fn(&mut MapPanel, usize, &mut Context<MapPanel>) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let entity = cx.entity().downgrade();
        let apply: std::rc::Rc<Pick> = std::rc::Rc::new(apply);
        Button::new(id).outline().xsmall().label(current).dropdown_caret(true).dropdown_menu(move |mut menu, _, _| {
            for (i, name) in items.iter().enumerate() {
                let (e, f) = (entity.clone(), apply.clone());
                menu = menu.item(PopupMenuItem::new(name.clone()).checked(checked == Some(i)).on_click(
                    move |_, _, cx| {
                        e.update(cx, |this, cx| f(this, i, cx)).ok();
                    },
                ));
            }
            menu
        })
        .into_any_element()
    }

    fn stepper(
        &self,
        id: &'static str,
        text: String,
        dec: impl Fn(&mut LayerStyle) + 'static,
        inc: impl Fn(&mut LayerStyle) + 'static,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Div {
        h_flex()
            .gap_1()
            .items_center()
            .child(Button::new((id, 0usize)).ghost().xsmall().label("\u{2212}").on_click(
                cx.listener(move |this, _, _, cx| this.edit(&dec, cx)),
            ))
            .child(div().min_w(px(52.)).flex().justify_center().text_size(px(12.)).child(text))
            .child(Button::new((id, 1usize)).ghost().xsmall().label("+").on_click(
                cx.listener(move |this, _, _, cx| this.edit(&inc, cx)),
            ))
    }

    fn toggle(
        &self,
        id: &'static str,
        label: &str,
        on: bool,
        flip: impl Fn(&mut LayerStyle) + 'static,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        h_flex()
            .id(id)
            .gap_2()
            .items_center()
            .h(px(24.))
            .rounded(px(4.))
            .cursor_pointer()
            .hover(|s| s.bg(t.hover))
            .on_click(cx.listener(move |this, _, _, cx| this.edit(&flip, cx)))
            .child(
                div()
                    .size(px(14.))
                    .rounded(px(3.))
                    .border_1()
                    .border_color(if on { t.accent } else { t.border })
                    .bg(if on { t.accent } else { t.bg })
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(on, |d| d.child(Icon::new(IconName::Check).size(px(10.)).text_color(t.bg))),
            )
            .child(div().text_size(px(12.)).text_color(t.fg).child(label.to_string()))
            .into_any_element()
    }

    /// The popover, anchored at the top-right of the map area.
    pub(crate) fn layers_popover(&self, t: &Theme, size: [f64; 2], cx: &mut Context<Self>) -> AnyElement {
        let core = self.core.borrow();
        let sel = self.sel_layer.min(core.layers.len().saturating_sub(1));
        let style = core.style(sel).clone();
        let stats = core.ls.stats.get(sel).cloned().unwrap_or_default();
        let rt = core.ls.rt.get(sel).cloned().unwrap_or_default();
        let numeric: Vec<String> = stats.iter().filter(|c| c.numeric).map(|c| c.name.clone()).collect();
        let all_cols: Vec<String> = stats.iter().map(|c| c.name.clone()).collect();
        let tiled = tiling_enabled(&style.tiling, rt.features, rt.vertices);
        let layer_rows: Vec<AnyElement> = core
            .layers
            .iter()
            .enumerate()
            .map(|(i, l)| {
                let c = crate::palette::layer_color(i, true);
                let swatch = hsla(c.h, c.s, c.l, 1.0);
                let visible = l.visible;
                let selected = i == sel;
                h_flex()
                    .gap_2()
                    .items_center()
                    .px_2()
                    .h(px(26.))
                    .rounded(px(4.))
                    .when(selected, |d| d.bg(t.hover))
                    .child(
                        div()
                            .id(("map-layer-vis", i))
                            .size(px(14.))
                            .rounded(px(3.))
                            .border_1()
                            .cursor_pointer()
                            .border_color(if visible { t.accent } else { t.border })
                            .bg(if visible { t.accent } else { t.bg })
                            .flex()
                            .items_center()
                            .justify_center()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(l) = this.core.borrow_mut().layers.get_mut(i) {
                                    l.visible = !l.visible;
                                }
                                cx.notify();
                            }))
                            .when(visible, |d| d.child(Icon::new(IconName::Check).size(px(10.)).text_color(t.bg))),
                    )
                    .child(
                        h_flex()
                            .id(("map-layer", i))
                            .flex_1()
                            .min_w_0()
                            .gap_2()
                            .items_center()
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.sel_layer = i;
                                cx.notify();
                            }))
                            .child(div().size(px(9.)).rounded_full().flex_shrink_0().bg(swatch))
                            .child(
                                div().flex_1().min_w_0().truncate().text_size(px(12.)).text_color(t.fg).child(l.name.clone()),
                            )
                            .child(muted_text(t, core.layer_counts.get(i).copied().unwrap_or(0).to_string())),
                    )
                    .into_any_element()
            })
            .collect();
        drop(core);

        let kind_items: Vec<String> = LayerKind::ALL.iter().map(|k| k.label().to_string()).collect();
        let kind_idx = LayerKind::ALL.iter().position(|k| *k == style.kind);
        let mut body = v_flex().gap_0p5().child(row(
            t,
            "Kind",
            self.dropdown(
                "map-kind",
                style.kind.label().to_string(),
                kind_items,
                kind_idx,
                |this, i, cx| this.edit(|s| s.kind = LayerKind::ALL[i], cx),
                cx,
            ),
        ));

        match style.kind {
            LayerKind::Auto | LayerKind::Points => {
                body = body.child(self.colour_rows(t, &style, &all_cols, &numeric, cx));
                if style.kind == LayerKind::Points || !rt.only_points {
                    body = body.child(row(
                        t,
                        "Outline",
                        self.stepper(
                            "map-stroke",
                            format!("{:.1} px", style.stroke_width),
                            |s| s.stroke_width = (s.stroke_width - 0.5).max(0.0),
                            |s| s.stroke_width = (s.stroke_width + 0.5).min(8.0),
                            cx,
                        ),
                    ));
                }
                if rt.only_points || style.kind == LayerKind::Points {
                    body = body.child(row(
                        t,
                        "Radius",
                        self.stepper(
                            "map-radius",
                            format!("{:.1} px", style.points.radius_px),
                            |s| s.points.radius_px = (s.points.radius_px - 0.5).max(0.5),
                            |s| s.points.radius_px = (s.points.radius_px + 0.5).min(40.0),
                            cx,
                        ),
                    ));
                    let mut items = vec!["None".to_string()];
                    items.extend(numeric.iter().cloned());
                    let cur = style.points.radius_by.as_ref().map(|r| r.column.clone());
                    let checked = cur.as_ref().and_then(|c| items.iter().position(|i| i == c)).or(Some(0));
                    let names = numeric.clone();
                    body = body.child(row(
                        t,
                        "Size by",
                        self.dropdown(
                            "map-size-by",
                            cur.clone().unwrap_or_else(|| "None".into()),
                            items,
                            checked,
                            move |this, i, cx| {
                                let col = (i > 0).then(|| names[i - 1].clone());
                                this.edit(
                                    |s| {
                                        s.points.radius_by =
                                            col.map(|column| RadiusBy { column, min_px: 2.0, max_px: 16.0 })
                                    },
                                    cx,
                                )
                            },
                            cx,
                        ),
                    ));
                }
                body = body.child(self.opacity_row(t, &style, cx));
                body = body.child(self.tiling_rows(t, &style, tiled, rt.only_points, cx));
            }
            LayerKind::Heatmap => {
                body = body
                    .child(row(
                        t,
                        "Radius",
                        self.stepper(
                            "map-heat-radius",
                            format!("{:.0} px", style.heatmap.radius_px),
                            |s| s.heatmap.radius_px = (s.heatmap.radius_px - 4.0).max(2.0),
                            |s| s.heatmap.radius_px = (s.heatmap.radius_px + 4.0).min(120.0),
                            cx,
                        ),
                    ))
                    .child(row(
                        t,
                        "Intensity",
                        self.stepper(
                            "map-heat-intensity",
                            format!("{:.1}", style.heatmap.intensity),
                            |s| s.heatmap.intensity = (s.heatmap.intensity - 0.2).max(0.1),
                            |s| s.heatmap.intensity = (s.heatmap.intensity + 0.2).min(10.0),
                            cx,
                        ),
                    ))
                    .child(row(
                        t,
                        "Threshold",
                        self.stepper(
                            "map-heat-threshold",
                            format!("{:.2}", style.heatmap.threshold),
                            |s| s.heatmap.threshold = (s.heatmap.threshold - 0.02).max(0.0),
                            |s| s.heatmap.threshold = (s.heatmap.threshold + 0.02).min(0.9),
                            cx,
                        ),
                    ));
                let mut items = vec!["None (count)".to_string()];
                items.extend(numeric.iter().cloned());
                let cur = style.heatmap.weight.clone();
                let checked = cur.as_ref().and_then(|c| items.iter().position(|i| i == c)).or(Some(0));
                let names = numeric.clone();
                body = body
                    .child(row(
                        t,
                        "Weight",
                        self.dropdown(
                            "map-heat-weight",
                            cur.clone().unwrap_or_else(|| "None (count)".into()),
                            items,
                            checked,
                            move |this, i, cx| {
                                let col = (i > 0).then(|| names[i - 1].clone());
                                this.edit(|s| s.heatmap.weight = col, cx)
                            },
                            cx,
                        ),
                    ))
                    .child(self.palette_row(t, "heat", style.heatmap.palette, style.heatmap.reversed, true, cx))
                    .child(self.opacity_row(t, &style, cx));
            }
            LayerKind::Hexagons | LayerKind::Grid => {
                let b = &style.bins;
                body = body.child(row(
                    t,
                    "Cell size",
                    self.stepper(
                        "map-cell",
                        format!("{:.0} px", b.cell_px),
                        |s| s.bins.cell_px = (s.bins.cell_px - 4.0).max(8.0),
                        |s| s.bins.cell_px = (s.bins.cell_px + 4.0).min(200.0),
                        cx,
                    ),
                ));
                let vals: Vec<String> = BinValue::ALL.iter().map(|v| v.label().to_string()).collect();
                let vi = BinValue::ALL.iter().position(|v| *v == b.value);
                let numeric2 = numeric.clone();
                body = body.child(row(
                    t,
                    "Value",
                    self.dropdown(
                        "map-bin-value",
                        b.value.label().to_string(),
                        vals,
                        vi,
                        move |this, i, cx| {
                            let v = BinValue::ALL[i];
                            let first = numeric2.first().cloned();
                            this.edit(
                                |s| {
                                    s.bins.value = v;
                                    if v != BinValue::Count && s.bins.column.is_none() {
                                        s.bins.column = first;
                                    }
                                    if v == BinValue::Count {
                                        s.bins.column = None;
                                    }
                                },
                                cx,
                            )
                        },
                        cx,
                    ),
                ));
                if b.value != BinValue::Count {
                    let names = numeric.clone();
                    let checked = b.column.as_ref().and_then(|c| names.iter().position(|n| n == c));
                    body = body.child(row(
                        t,
                        "Column",
                        self.dropdown(
                            "map-bin-column",
                            b.column.clone().unwrap_or_else(|| "No numeric column".into()),
                            numeric.clone(),
                            checked,
                            move |this, i, cx| {
                                let c = names[i].clone();
                                this.edit(|s| s.bins.column = Some(c), cx)
                            },
                            cx,
                        ),
                    ));
                }
                let methods: Vec<String> = Method::ALL.iter().map(|m| m.label().to_string()).collect();
                let mi = Method::ALL.iter().position(|m| *m == b.method);
                body = body
                    .child(row(
                        t,
                        "Classes",
                        h_flex().gap_1().items_center().child(self.dropdown(
                            "map-bin-method",
                            b.method.label().to_string(),
                            methods,
                            mi,
                            |this, i, cx| this.edit(|s| s.bins.method = Method::ALL[i], cx),
                            cx,
                        )),
                    ))
                    .child(row(
                        t,
                        "Class count",
                        self.stepper(
                            "map-bin-classes",
                            b.classes.to_string(),
                            |s| s.bins.classes = s.bins.classes.saturating_sub(1).max(5),
                            |s| s.bins.classes = (s.bins.classes + 1).min(9),
                            cx,
                        ),
                    ))
                    .child(self.palette_row(t, "bins", b.palette, b.reversed, false, cx))
                    .child(self.opacity_row(t, &style, cx));
            }
            LayerKind::Clusters => {
                body = body
                    .child(row(
                        t,
                        "Radius",
                        self.stepper(
                            "map-cluster-radius",
                            format!("{:.0} px", style.clusters.radius_px),
                            |s| s.clusters.radius_px = (s.clusters.radius_px - 8.0).max(16.0),
                            |s| s.clusters.radius_px = (s.clusters.radius_px + 8.0).min(160.0),
                            cx,
                        ),
                    ))
                    .child(self.opacity_row(t, &style, cx))
                    .child(muted_text(t, "Click a cluster to zoom in."));
            }
        }

        floating(
            [(size[0] - (f64::from(W) + 8.0)).max(0.0), 6.0],
            v_flex()
                .w(px(W))
                .p_1()
                .rounded(px(6.))
                .border_1()
                .border_color(t.border)
                .bg(t.popover)
                .shadow_md()
                .occlude()
                .child(
                    v_flex()
                        .id("map-layers-panel")
                        .max_h(px((size[1] - 24.0).max(160.0) as f32))
                        .overflow_y_scroll()
                        .gap_0p5()
                        .child(div().px_2().py_0p5().child(muted_text(t, "Layers")))
                        .children(layer_rows)
                        .child(div().px_2().pb_1().child(section(t, "Style").child(body))),
                ),
        )
    }

    fn opacity_row(&self, t: &Theme, s: &LayerStyle, cx: &mut Context<Self>) -> gpui_kit::Div {
        row(
            t,
            "Opacity",
            self.stepper(
                "map-opacity",
                format!("{:.0}%", s.opacity * 100.0),
                |s| s.opacity = ((s.opacity * 100.0 - 10.0).round() / 100.0).max(0.1),
                |s| s.opacity = ((s.opacity * 100.0 + 10.0).round() / 100.0).min(1.0),
                cx,
            ),
        )
    }

    fn palette_row(
        &self,
        t: &Theme,
        id: &'static str,
        palette: PaletteId,
        reversed: bool,
        heat: bool,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Div {
        let items: Vec<String> = PaletteId::ALL.iter().map(|p| p.label().to_string()).collect();
        let pi = PaletteId::ALL.iter().position(|p| *p == palette);
        row(
            t,
            "Palette",
            h_flex()
                .gap_1()
                .items_center()
                .child(self.dropdown(
                    (id, 7usize),
                    palette.label().to_string(),
                    items,
                    pi,
                    move |this, i, cx| {
                        let p = PaletteId::ALL[i];
                        this.edit(
                            |s| {
                                if heat {
                                    s.heatmap.palette = p
                                } else {
                                    s.bins.palette = p
                                }
                            },
                            cx,
                        )
                    },
                    cx,
                ))
                .child(
                    Button::new((id, 8usize))
                        .ghost()
                        .xsmall()
                        .label(if reversed { "Reversed" } else { "Reverse" })
                        .selected(reversed)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.edit(
                                |s| {
                                    if heat {
                                        s.heatmap.reversed = !s.heatmap.reversed
                                    } else {
                                        s.bins.reversed = !s.bins.reversed
                                    }
                                },
                                cx,
                            )
                        })),
                ),
        )
    }

    fn colour_rows(
        &self,
        t: &Theme,
        style: &LayerStyle,
        all_cols: &[String],
        numeric: &[String],
        cx: &mut Context<Self>,
    ) -> gpui_kit::Div {
        let mut items = vec!["Single colour".to_string()];
        items.extend(all_cols.iter().cloned());
        let cur = match &style.color {
            ColorSpec::ByColumn(c) => Some(c.column.clone()),
            ColorSpec::Single(_) => None,
        };
        let checked = cur.as_ref().and_then(|c| items.iter().position(|i| i == c)).or(Some(0));
        let (names, numeric) = (all_cols.to_vec(), numeric.to_vec());
        let mut out = v_flex().gap_0p5().child(row(
            t,
            "Colour by",
            self.dropdown(
                "map-colour-by",
                cur.clone().unwrap_or_else(|| "Single colour".into()),
                items,
                checked,
                move |this, i, cx| {
                    if i == 0 {
                        this.edit(|s| s.color = ColorSpec::Single(None), cx);
                        return;
                    }
                    let column = names[i - 1].clone();
                    let by = if numeric.contains(&column) {
                        ColorBy {
                            column,
                            scheme: ColorScheme::Numeric { method: Method::Quantile, classes: 7 },
                            palette: PaletteId::Viridis,
                            reversed: false,
                        }
                    } else {
                        ColorBy {
                            column,
                            scheme: ColorScheme::Categorical { top_n: 8 },
                            palette: PaletteId::Tableau10,
                            reversed: false,
                        }
                    };
                    this.edit(|s| s.color = ColorSpec::ByColumn(by), cx)
                },
                cx,
            ),
        ));
        let ColorSpec::ByColumn(by) = &style.color else { return out };
        let palette_items: Vec<String> = PaletteId::ALL.iter().map(|p| p.label().to_string()).collect();
        let pi = PaletteId::ALL.iter().position(|p| *p == by.palette);
        match by.scheme {
            ColorScheme::Numeric { method, classes } => {
                let methods: Vec<String> = Method::ALL.iter().map(|m| m.label().to_string()).collect();
                let mi = Method::ALL.iter().position(|m| *m == method);
                out = out
                    .child(row(
                        t,
                        "Classes",
                        self.dropdown(
                            "map-colour-method",
                            method.label().to_string(),
                            methods,
                            mi,
                            |this, i, cx| {
                                this.edit(
                                    |s| {
                                        if let ColorSpec::ByColumn(c) = &mut s.color
                                            && let ColorScheme::Numeric { method, .. } = &mut c.scheme
                                        {
                                            *method = Method::ALL[i];
                                        }
                                    },
                                    cx,
                                )
                            },
                            cx,
                        ),
                    ))
                    .child(row(
                        t,
                        "Class count",
                        self.stepper(
                            "map-colour-classes",
                            classes.to_string(),
                            |s| {
                                if let ColorSpec::ByColumn(c) = &mut s.color
                                    && let ColorScheme::Numeric { classes, .. } = &mut c.scheme
                                {
                                    *classes = classes.saturating_sub(1).max(5);
                                }
                            },
                            |s| {
                                if let ColorSpec::ByColumn(c) = &mut s.color
                                    && let ColorScheme::Numeric { classes, .. } = &mut c.scheme
                                {
                                    *classes = (*classes + 1).min(9);
                                }
                            },
                            cx,
                        ),
                    ));
            }
            ColorScheme::Categorical { top_n } => {
                out = out.child(row(
                    t,
                    "Top values",
                    self.stepper(
                        "map-colour-top",
                        top_n.to_string(),
                        |s| {
                            if let ColorSpec::ByColumn(c) = &mut s.color
                                && let ColorScheme::Categorical { top_n } = &mut c.scheme
                            {
                                *top_n = top_n.saturating_sub(1).max(2);
                            }
                        },
                        |s| {
                            if let ColorSpec::ByColumn(c) = &mut s.color
                                && let ColorScheme::Categorical { top_n } = &mut c.scheme
                            {
                                *top_n = (*top_n + 1).min(10);
                            }
                        },
                        cx,
                    ),
                ));
            }
        }
        out.child(row(
            t,
            "Palette",
            h_flex()
                .gap_1()
                .items_center()
                .child(self.dropdown(
                    "map-colour-palette",
                    by.palette.label().to_string(),
                    palette_items,
                    pi,
                    |this, i, cx| {
                        this.edit(
                            |s| {
                                if let ColorSpec::ByColumn(c) = &mut s.color {
                                    c.palette = PaletteId::ALL[i];
                                }
                            },
                            cx,
                        )
                    },
                    cx,
                ))
                .child(
                    Button::new("map-colour-reverse")
                        .ghost()
                        .xsmall()
                        .label(if by.reversed { "Reversed" } else { "Reverse" })
                        .selected(by.reversed)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.edit(
                                |s| {
                                    if let ColorSpec::ByColumn(c) = &mut s.color {
                                        c.reversed = !c.reversed;
                                    }
                                },
                                cx,
                            )
                        })),
                ),
        ))
    }

    fn tiling_rows(
        &self,
        t: &Theme,
        style: &LayerStyle,
        tiled: bool,
        only_points: bool,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Div {
        let tl = &style.tiling;
        let modes = vec!["Auto".to_string(), "On".to_string(), "Off".to_string()];
        let mi = [TilingMode::Auto, TilingMode::On, TilingMode::Off].iter().position(|m| *m == tl.mode);
        let label = match tl.mode {
            TilingMode::Auto => format!("Auto ({})", if tiled { "on" } else { "off" }),
            TilingMode::On => "On".into(),
            TilingMode::Off => "Off".into(),
        };
        let mut out = v_flex().gap_0p5().child(section(t, "Tiling")).child(row(
            t,
            "Tiles",
            self.dropdown(
                "map-tiling-mode",
                label,
                modes,
                mi,
                |this, i, cx| {
                    this.edit(|s| s.tiling.mode = [TilingMode::Auto, TilingMode::On, TilingMode::Off][i], cx)
                },
                cx,
            ),
        ));
        if !tiled {
            return out.child(muted_text(t, "Drawn from the full data; tiles start above 20,000 features."));
        }
        out = out.child(row(
            t,
            "Detail",
            self.stepper(
                "map-tiling-detail",
                format!("{:.1} px", tl.detail_px),
                |s| s.tiling.detail_px = (s.tiling.detail_px - 0.2).max(0.0),
                |s| s.tiling.detail_px = (s.tiling.detail_px + 0.2).min(8.0),
                cx,
            ),
        ));
        let algos: Vec<String> = Simplify::ALL.iter().map(|a| a.label().to_string()).collect();
        let ai = Simplify::ALL.iter().position(|a| *a == tl.simplify);
        out = out.child(row(
            t,
            "Simplify",
            self.dropdown(
                "map-tiling-algo",
                tl.simplify.label().to_string(),
                algos,
                ai,
                |this, i, cx| this.edit(|s| s.tiling.simplify = Simplify::ALL[i], cx),
                cx,
            ),
        ));
        if only_points {
            out = out.child(row(
                t,
                "Point budget",
                self.stepper(
                    "map-tiling-budget",
                    format!("{}", tl.point_budget),
                    |s| s.tiling.point_budget = (s.tiling.point_budget / 2).max(500),
                    |s| s.tiling.point_budget = (s.tiling.point_budget * 2).min(500_000),
                    cx,
                ),
            ));
        }
        out.child(self.toggle("map-tiling-tiny", "Drop tiny polygons", tl.drop_tiny_polygons, |s| s.tiling.drop_tiny_polygons = !s.tiling.drop_tiny_polygons, t, cx))
            .child(self.toggle("map-tiling-all", "Keep every feature", tl.keep_every_feature, |s| s.tiling.keep_every_feature = !s.tiling.keep_every_feature, t, cx))
            .child(self.toggle("map-tiling-bounds", "Show tile boundaries", tl.show_boundaries, |s| s.tiling.show_boundaries = !s.tiling.show_boundaries, t, cx))
    }
}
