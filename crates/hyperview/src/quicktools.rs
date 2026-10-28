//! Quick tools: a short bar down the right-hand side of the window holding the
//! few tools each person uses all day, in the order they want them.
//!
//! It is there in simple mode and with the full toolbars alike. Each person
//! picks their own from every tool the program has (View, Choose Quick
//! Tools), puts them in order by dragging or with the arrows, and the choice
//! is kept in this computer's settings. The office server keeps no settings
//! for each person, so the choice stays with the computer it was made on.

use egui::RichText;

use crate::app::App;

/// The tools a new seat starts with: what a fabricator's estimator measures
/// and marks up most.
pub const DEFAULTS: &[&str] = &[
    "Measure.Length", // WHAT: length. WHY: most of a takeoff is lengths.
    "Measure.Area", // WHAT: area. WHY: decks, slabs and plate.
    "Measure.Count", // WHAT: count. WHY: embeds, anchors, columns.
    "Markup.Cloud", // WHAT: a revision cloud. WHY: marking what changed.
    "Markup.TextBox", // WHAT: a text box. WHY: a note on the sheet.
    "Markup.Pen", // WHAT: the pen. WHY: a quick sketch or circle, by hand.
];

/// What the quick tools bar is doing, beyond what the settings hold.
#[derive(Debug, Default)]
pub struct Quick {
    /// The list being set up in the Choose Quick Tools window, while it is
    /// open. Saved when Done is pressed; Cancel leaves the old one.
    pub choosing: Option<Vec<String>>, // WHAT: the list being edited. WHY: changes are tried out before they are kept.
}

/// Every tool that can go in the bar: each command that puts a tool in the
/// hand, in the order the program lists them, each once.
pub fn choosable() -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new(); // WHAT: the tools found. WHY: built up from every command.
    for command in ui::command::ALL {
        let real = crate::panelbody::tool_for(command.id).is_some(); // WHAT: a command that puts a tool in the hand. WHY: Set Scale is a button rather than a toggle, and is still a tool; a button that did nothing would be worse than none.
        if real && !out.contains(&command.id) {
            out.push(command.id); // WHAT: kept. WHY: each tool once.
        }
    }
    out
}

/// The tools in the bar: the ones chosen, or the usual six. Anything chosen
/// that this version has no tool for is left out, and each tool comes once.
pub fn tools_of(saved: Option<&Vec<String>>) -> Vec<String> {
    let all = choosable(); // WHAT: every tool there is. WHY: what is kept has to be one of them.
    let wanted: Vec<String> = match saved {
        Some(list) => list.clone(), // WHAT: the person's own. WHY: theirs wins.
        None => DEFAULTS.iter().map(|s| s.to_string()).collect(), // WHAT: the usual six. WHY: nothing chosen yet.
    };
    let mut out: Vec<String> = Vec::new(); // WHAT: what goes in the bar. WHY: built from what was wanted.
    for id in wanted {
        if all.contains(&id.as_str()) && !out.contains(&id) {
            out.push(id); // WHAT: a real tool, not already there. WHY: settings from another version or edited by hand can hold either.
        }
    }
    out
}

/// Moves the tool at `from` to `to`, the others closing up behind it.
pub fn move_to(list: &mut Vec<String>, from: usize, to: usize) {
    if from >= list.len() || to >= list.len() || from == to {
        return; // WHAT: nothing to move. WHY: a drag that lands where it started, or off the end.
    }
    let id = list.remove(from); // WHAT: lift it out. WHY: the rest close up.
    list.insert(to, id); // WHAT: put it where it was dropped. WHY: that is the order wanted.
}

/// Adds a tool to the end of the bar, unless it is already in it.
pub fn add(list: &mut Vec<String>, id: &str) {
    if !list.iter().any(|t| t == id) {
        list.push(id.to_string()); // WHAT: on the end. WHY: each tool once.
    }
}

/// Which group a tool is listed under in the chooser.
pub fn group_of(id: &str) -> &'static str {
    if id.starts_with("Measure.") {
        "Measure" // WHAT: measuring tools. WHY: listed together, as on the Measure menu.
    } else if id.starts_with("Form.") {
        "Forms" // WHAT: form fields. WHY: rarely wanted, so kept apart.
    } else if id.starts_with("Markup.") || matches!(id, "Eraser" | "Snapshot") {
        "Markup" // WHAT: drawing and marking tools. WHY: listed together, as on the Markup menu.
    } else {
        "Getting round" // WHAT: select, pan, lasso and the like. WHY: the rest.
    }
}

impl App {
    /// The tools in the quick tools bar now.
    pub fn quick_tools(&self) -> Vec<String> {
        tools_of(self.prefs.quick_tools.as_ref()) // WHAT: from this computer's settings. WHY: kept per device.
    }

    /// The bar down the right-hand side, when it is shown.
    pub fn quick_tools_bar(&mut self, ctx: &egui::Context) {
        if !self.prefs.quick_tools_shown || self.doc().is_none() {
            return; // WHAT: no bar. WHY: put away, or no drawing to use the tools on.
        }
        let tools = self.quick_tools(); // WHAT: the tools to show. WHY: the person's own, in their order.
        let theme = self.chrome.theme; // WHAT: the colours in use. WHY: the bar matches the rest.
        egui::SidePanel::right("quick-tools")
            .frame(crate::panels::bar_frame(theme.chrome)) // WHAT: the same frame as the rail on the left. WHY: one look for both sides.
            .exact_width(ui::chrome::BUTTON + 14.0) // WHAT: one button wide. WHY: takes as little of the drawing as it can.
            .resizable(false) // WHAT: a fixed width. WHY: there is nothing to widen it for.
            .show(ctx, |ui| {
                ui.add_space(6.0); // WHAT: a little room at the top. WHY: lines up with the rail.
                ui.vertical_centered(|ui| {
                    ui.spacing_mut().item_spacing.y = 4.0; // WHAT: a gap between buttons. WHY: easier to hit with a finger.
                    for id in &tools {
                        self.chrome.button(ui, id); // WHAT: the same button the toolbars have. WHY: it lights up, picks the tool and explains itself the same way.
                    }
                    ui.add_space(10.0); // WHAT: a gap before the set-up button. WHY: so it is not taken for a tool.
                    self.chrome.button(ui, "View.ChooseQuickTools"); // WHAT: set the bar up. WHY: from the bar itself, where somebody looks for it.
                });
            });
    }

    /// Opens the Choose Quick Tools window on the tools in the bar now.
    pub fn choose_quick_tools(&mut self) {
        self.quick.choosing = Some(self.quick_tools()); // WHAT: a copy to edit. WHY: Cancel leaves the bar as it was.
    }

    /// The Choose Quick Tools window: the bar's tools in order on the left,
    /// to drag or move with the arrows and take off; every tool on the right,
    /// to add.
    pub fn quick_tools_window(&mut self, ctx: &egui::Context) {
        let Some(mut list) = self.quick.choosing.take() else { return }; // WHAT: the list being edited. WHY: no window unless it is open.
        let theme = self.chrome.theme; // WHAT: the colours in use. WHY: matches the rest.
        let (mut keep, mut save) = (true, false); // WHAT: whether the window stays and whether to keep the list. WHY: acted on after drawing.
        let mut moved: Option<(usize, usize)> = None; // WHAT: a tool moved, from where to where. WHY: the list cannot change while it is drawn.
        let mut dropped: Option<usize> = None; // WHAT: a tool taken off. WHY: the same.
        let mut added: Option<&'static str> = None; // WHAT: a tool added. WHY: the same.
        let all = choosable(); // WHAT: every tool there is. WHY: the right-hand list.
        egui::Window::new("Choose Quick Tools")
            .collapsible(false) // WHAT: no folding it up. WHY: it is a short task, finished with Done.
            .resizable(true) // WHAT: can be made taller. WHY: the list of every tool is long.
            .default_width(560.0) // WHAT: room for both lists. WHY: side by side reads best.
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0)) // WHAT: in the middle. WHY: where a window that wants an answer goes.
            .show(ctx, |ui| {
                ui.label(
                    RichText::new("The tools in the bar down the right-hand side, top to bottom. Drag them into order, or use the arrows. They are kept on this computer.")
                        .color(theme.faint)
                        .size(11.0),
                ); // WHAT: what this is. WHY: one sentence is enough to start.
                ui.add_space(6.0);
                ui.columns(2, |cols| {
                    cols[0].label(RichText::new("In the bar").strong()); // WHAT: the left list's heading. WHY: says which list is which.
                    if list.is_empty() {
                        cols[0].label(RichText::new("Nothing yet. Add tools from the list on the right.").color(theme.faint).size(11.0)); // WHAT: an empty bar explained. WHY: so nobody thinks it broke.
                    }
                    let last = list.len().saturating_sub(1); // WHAT: the last place. WHY: it cannot move further down.
                    for (i, id) in list.iter().enumerate() {
                        let row = cols[0].horizontal(|ui| {
                            if ui.add_enabled(i > 0, egui::Button::new("⏶").small()).on_hover_text("Higher in the bar").clicked() {
                                moved = Some((i, i - 1)); // WHAT: up one. WHY: for somebody who would rather not drag.
                            }
                            if ui.add_enabled(i < last, egui::Button::new("⏷").small()).on_hover_text("Lower in the bar").clicked() {
                                moved = Some((i, i + 1)); // WHAT: down one. WHY: the same.
                            }
                            if ui.button("×").on_hover_text("Take it out of the bar").clicked() {
                                dropped = Some(i); // WHAT: off the bar. WHY: not wanted there.
                            }
                            ui.dnd_drag_source(egui::Id::new(("quick-tool", i)), i, |ui| {
                                ui.label(ui::command::label_of(id)).on_hover_text("Drag onto another tool to move it there"); // WHAT: the tool's name, which can be picked up. WHY: says on hover that it can be dragged.
                            }); // WHAT: the name can be picked up and dropped on another row. WHY: dragging is the quickest way to reorder.
                        });
                        if let Some(from) = row.response.dnd_release_payload::<usize>() {
                            moved = Some((*from, i)); // WHAT: a dragged tool let go on this row. WHY: it goes here.
                        }
                    }
                    cols[1].label(RichText::new("Every tool").strong()); // WHAT: the right list's heading. WHY: says which list is which.
                    egui::ScrollArea::vertical().max_height(360.0).show(&mut cols[1], |ui| {
                        let mut heading = ""; // WHAT: the group last headed. WHY: a heading each time the group changes.
                        let mut ordered: Vec<&'static str> = all.clone(); // WHAT: every tool. WHY: sorted into groups for reading.
                        ordered.sort_by_key(|id| match group_of(id) { "Measure" => 0, "Markup" => 1, "Getting round" => 2, _ => 3 }); // WHAT: measuring first, forms last. WHY: the order they are wanted in.
                        for id in ordered {
                            let group = group_of(id); // WHAT: this tool's group. WHY: for the heading.
                            if group != heading {
                                ui.add_space(4.0);
                                ui.label(RichText::new(group).color(theme.faint).size(11.0)); // WHAT: the group's name. WHY: a long list reads better in parts.
                                heading = group;
                            }
                            let there = list.iter().any(|t| t == id); // WHAT: already in the bar. WHY: each tool once.
                            ui.horizontal(|ui| {
                                if ui.add_enabled(!there, egui::Button::new("Add").small()).clicked() {
                                    added = Some(id); // WHAT: add it. WHY: wanted in the bar.
                                }
                                ui.label(ui::command::label_of(id)); // WHAT: the tool's name. WHY: what is being added.
                            });
                        }
                    });
                });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Done").clicked() {
                        save = true; // WHAT: keep this list. WHY: finished.
                        keep = false;
                    }
                    if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        keep = false; // WHAT: leave the bar as it was. WHY: changed their mind.
                    }
                    if ui.button("Back to the usual six").on_hover_text("Length, area, count, cloud, text box and pen").clicked() {
                        list = DEFAULTS.iter().map(|s| s.to_string()).collect(); // WHAT: the starting tools again. WHY: an easy way back.
                    }
                });
            });
        if let Some((from, to)) = moved {
            move_to(&mut list, from, to); // WHAT: reorder. WHY: an arrow pressed or a tool dropped.
        }
        if let Some(i) = dropped {
            if i < list.len() {
                list.remove(i); // WHAT: take it out. WHY: × pressed.
            }
        }
        if let Some(id) = added {
            add(&mut list, id); // WHAT: put it on the end. WHY: Add pressed.
        }
        if save {
            self.prefs.quick_tools = Some(list); // WHAT: this computer keeps the list. WHY: the office server keeps no settings for each person.
            self.prefs.quick_tools_shown = true; // WHAT: the bar shown. WHY: somebody who just set it up wants to see it.
            if let Err(e) = self.prefs.save() {
                self.error = Some(format!("Could not keep the quick tools: {e}")); // WHAT: say it was not kept. WHY: better than finding out next time.
            }
        } else if keep {
            self.quick.choosing = Some(list); // WHAT: still open. WHY: carries on next frame.
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_seat_has_length_area_count_cloud_text_and_pen() {
        let tools = tools_of(None); // WHAT: nothing chosen. WHY: a first run.
        assert_eq!(tools, DEFAULTS.iter().map(|s| s.to_string()).collect::<Vec<_>>()); // WHAT: the usual six, in order. WHY: what was asked for.
    }

    #[test]
    fn every_default_and_every_choosable_tool_is_a_real_tool() {
        for id in DEFAULTS {
            assert!(crate::panelbody::tool_for(id).is_some(), "{id} is not a tool"); // WHAT: each default picks a tool. WHY: a dead button in the bar would be worse than none.
        }
        let all = choosable(); // WHAT: every tool offered. WHY: the chooser's list.
        assert!(all.len() > 30, "only {} tools offered", all.len()); // WHAT: the full set. WHY: anybody may want any of them.
        assert!(all.contains(&"Measure.Calibrate") && all.contains(&"Select")); // WHAT: two that must be there. WHY: setting the scale and picking things up.
        assert!(!all.contains(&"View.SimpleMode")); // WHAT: a setting is not a tool. WHY: only tools go in the bar.
    }

    #[test]
    fn a_kept_list_loses_what_this_version_has_no_tool_for_and_any_repeats() {
        let saved = vec!["Markup.Pen".to_string(), "Markup.Unheardof".into(), "Markup.Pen".into(), "Measure.Angle".into()]; // WHAT: a list with a stranger and a repeat. WHY: a hand-edited or newer settings file.
        assert_eq!(tools_of(Some(&saved)), vec!["Markup.Pen".to_string(), "Measure.Angle".into()]); // WHAT: only real tools, once each, in order. WHY: no dead or doubled buttons.
        assert!(tools_of(Some(&Vec::new())).is_empty()); // WHAT: an empty list chosen. WHY: is kept empty, not replaced by the six.
    }

    #[test]
    fn tools_move_up_down_and_by_dragging_and_are_added_once() {
        let mut list: Vec<String> = ["a", "b", "c", "d"].iter().map(|s| s.to_string()).collect(); // WHAT: four in a row. WHY: room to move them.
        move_to(&mut list, 3, 0); // WHAT: the last dragged to the top. WHY: a drag.
        assert_eq!(list, ["d", "a", "b", "c"]); // WHAT: it is first, the rest closed up. WHY: the order wanted.
        move_to(&mut list, 0, 1); // WHAT: down one. WHY: the arrow.
        assert_eq!(list, ["a", "d", "b", "c"]); // WHAT: swapped with the next. WHY: one place down.
        move_to(&mut list, 2, 9); // WHAT: off the end. WHY: a drop outside the list.
        assert_eq!(list, ["a", "d", "b", "c"]); // WHAT: nothing changed. WHY: nowhere to go.
        add(&mut list, "b"); // WHAT: one already there. WHY: each tool once.
        add(&mut list, "e"); // WHAT: a new one. WHY: goes on the end.
        assert_eq!(list, ["a", "d", "b", "c", "e"]); // WHAT: added once. WHY: no repeats.
    }
}
