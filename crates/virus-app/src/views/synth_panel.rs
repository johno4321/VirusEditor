//! Synthesizer sound engine editing panel (Oscillators, Filters, Envelopes, LFOs, Effects).

use egui::{Color32, ComboBox, Frame, RichText, ScrollArea, Ui};
use virus_syx::domain::patch::Patch;
use virus_syx::domain::types::{
    FilterMode, FilterRouting, LfoShape, LfoTriggerMode, OscShape,
};

use crate::widgets::curves::{EnvelopeCurveWidget, FilterCurveWidget};
use crate::widgets::knob::{bipolar_knob, unipolar_knob};

pub struct SynthPanel;

impl SynthPanel {
    /// Renders the complete synthesizer sound engine editor.
    /// Returns a list of real-time parameter changes: `(page, param_number, value)`.
    pub fn show(ui: &mut Ui, patch: &mut Patch) -> Vec<(u8, u8, u8)> {
        let mut changes = Vec::new();

        ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(4.0);

                // --- Top Patch Name Bar ---
                ui.horizontal(|ui| {
                    ui.label(RichText::new("PATCH NAME:").strong().color(Color32::from_rgb(220, 70, 40)));
                    let mut name = patch.name();
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut name)
                            .desired_width(160.0)
                            .char_limit(10),
                    );
                    if response.changed() {
                        patch.set_name(&name);
                        // SysEx name characters update (offsets 118..127 on Page A)
                        for (i, b) in patch.page_a.raw[118..128].iter().enumerate() {
                            changes.push((0x70, (118 + i) as u8, *b));
                        }
                    }
                    ui.label(
                        RichText::new("(10 ASCII Characters)")
                            .size(11.0)
                            .color(Color32::from_rgb(120, 125, 140)),
                    );
                });

                ui.add_space(8.0);

                // --- 1. Oscillators & Mixer ---
                Frame::group(ui.style()).show(ui, |ui| {
                    ui.heading(RichText::new("1. Oscillators & Mixer").color(Color32::from_rgb(240, 200, 140)));
                    ui.add_space(4.0);

                    ui.horizontal_wrapped(|ui| {
                        // OSC 1
                        Frame::canvas(ui.style()).show(ui, |ui| {
                            ui.vertical(|ui| {
                                ui.label(RichText::new("OSC 1").strong());
                                ComboBox::from_id_source("osc1_shape")
                                    .selected_text(format!("{:?}", patch.page_a.osc1_shape))
                                    .show_ui(ui, |ui| {
                                        for shape in [OscShape::Saw, OscShape::Pulse, OscShape::Sine] {
                                            if ui.selectable_value(&mut patch.page_a.osc1_shape, shape, format!("{:?}", shape)).changed() {
                                                changes.push((0x70, 0, patch.page_a.osc1_shape.to_raw()));
                                            }
                                        }
                                    });

                                ui.horizontal(|ui| {
                                    if unipolar_knob(ui, &mut patch.page_a.osc1_wave_pw, "Wave/PW").changed() {
                                        changes.push((0x70, 1, patch.page_a.osc1_wave_pw.raw()));
                                    }
                                    if bipolar_knob(ui, &mut patch.page_a.osc1_semi, "Semitone").changed() {
                                        changes.push((0x70, 2, patch.page_a.osc1_semi.to_raw()));
                                    }
                                    if unipolar_knob(ui, &mut patch.page_a.osc1_detune, "Detune").changed() {
                                        changes.push((0x70, 3, patch.page_a.osc1_detune.raw()));
                                    }
                                    if bipolar_knob(ui, &mut patch.page_a.osc1_keyfollow, "Keyfollow").changed() {
                                        changes.push((0x70, 4, patch.page_a.osc1_keyfollow.to_raw()));
                                    }
                                });
                            });
                        });

                        ui.add_space(8.0);

                        // OSC 2
                        Frame::canvas(ui.style()).show(ui, |ui| {
                            ui.vertical(|ui| {
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new("OSC 2").strong());
                                    if ui.checkbox(&mut patch.page_a.osc_sync, "Sync").changed() {
                                        changes.push((0x70, 15, if patch.page_a.osc_sync { 1 } else { 0 }));
                                    }
                                });

                                ComboBox::from_id_source("osc2_shape")
                                    .selected_text(format!("{:?}", patch.page_a.osc2_shape))
                                    .show_ui(ui, |ui| {
                                        for shape in [OscShape::Saw, OscShape::Pulse, OscShape::Sine] {
                                            if ui.selectable_value(&mut patch.page_a.osc2_shape, shape, format!("{:?}", shape)).changed() {
                                                changes.push((0x70, 8, patch.page_a.osc2_shape.to_raw()));
                                            }
                                        }
                                    });

                                ui.horizontal(|ui| {
                                    if unipolar_knob(ui, &mut patch.page_a.osc2_wave_pw, "Wave/PW").changed() {
                                        changes.push((0x70, 9, patch.page_a.osc2_wave_pw.raw()));
                                    }
                                    if bipolar_knob(ui, &mut patch.page_a.osc2_semi, "Semitone").changed() {
                                        changes.push((0x70, 10, patch.page_a.osc2_semi.to_raw()));
                                    }
                                    if unipolar_knob(ui, &mut patch.page_a.osc2_detune, "Detune").changed() {
                                        changes.push((0x70, 11, patch.page_a.osc2_detune.raw()));
                                    }
                                    if unipolar_knob(ui, &mut patch.page_a.osc2_fm_amount, "FM Amt").changed() {
                                        changes.push((0x70, 13, patch.page_a.osc2_fm_amount.raw()));
                                    }
                                    if bipolar_knob(ui, &mut patch.page_a.pitch_env_amount, "Pitch Env").changed() {
                                        changes.push((0x70, 16, patch.page_a.pitch_env_amount.to_raw()));
                                    }
                                });
                            });
                        });

                        ui.add_space(8.0);

                        // Mixer & Noise
                        Frame::canvas(ui.style()).show(ui, |ui| {
                            ui.vertical(|ui| {
                                ui.label(RichText::new("Mixer / Noise").strong());
                                ui.horizontal(|ui| {
                                    if unipolar_knob(ui, &mut patch.page_a.sub_osc_volume, "Sub Vol").changed() {
                                        changes.push((0x70, 21, patch.page_a.sub_osc_volume.raw()));
                                    }
                                    if unipolar_knob(ui, &mut patch.page_a.noise_volume, "Noise Vol").changed() {
                                        changes.push((0x70, 24, patch.page_a.noise_volume.raw()));
                                    }
                                    if bipolar_knob(ui, &mut patch.page_a.noise_color, "Color").changed() {
                                        changes.push((0x70, 25, patch.page_a.noise_color.to_raw()));
                                    }
                                    if unipolar_knob(ui, &mut patch.page_a.ring_mod_volume, "RingMod").changed() {
                                        changes.push((0x70, 26, patch.page_a.ring_mod_volume.raw()));
                                    }
                                });
                            });
                        });
                    });
                });

                ui.add_space(8.0);

                // --- 2. Dual Filters & Visualizer ---
                Frame::group(ui.style()).show(ui, |ui| {
                    ui.heading(RichText::new("2. Dual Filters").color(Color32::from_rgb(240, 200, 140)));
                    ui.add_space(4.0);

                    ui.horizontal_wrapped(|ui| {
                        // Filter 1 Controls
                        Frame::canvas(ui.style()).show(ui, |ui| {
                            ui.vertical(|ui| {
                                ui.label(RichText::new("Filter 1").strong());
                                ComboBox::from_id_source("f1_mode")
                                    .selected_text(format!("{:?}", patch.page_a.filter1_mode))
                                    .show_ui(ui, |ui| {
                                        for mode in [FilterMode::Lowpass, FilterMode::Highpass, FilterMode::Bandpass, FilterMode::Bandstop] {
                                            if ui.selectable_value(&mut patch.page_a.filter1_mode, mode, format!("{:?}", mode)).changed() {
                                                changes.push((0x70, 32, patch.page_a.filter1_mode.to_raw()));
                                            }
                                        }
                                    });

                                ui.horizontal(|ui| {
                                    if unipolar_knob(ui, &mut patch.page_a.filter1_cutoff, "Cutoff").changed() {
                                        changes.push((0x70, 33, patch.page_a.filter1_cutoff.raw()));
                                    }
                                    if unipolar_knob(ui, &mut patch.page_a.filter1_resonance, "Resonance").changed() {
                                        changes.push((0x70, 34, patch.page_a.filter1_resonance.raw()));
                                    }
                                    if bipolar_knob(ui, &mut patch.page_a.filter1_env_amount, "Env Amt").changed() {
                                        changes.push((0x70, 35, patch.page_a.filter1_env_amount.to_raw()));
                                    }
                                    if bipolar_knob(ui, &mut patch.page_a.filter1_keyfollow, "Keyfollow").changed() {
                                        changes.push((0x70, 36, patch.page_a.filter1_keyfollow.to_raw()));
                                    }
                                });
                            });
                        });

                        ui.add_space(8.0);

                        // Filter 2 Controls
                        Frame::canvas(ui.style()).show(ui, |ui| {
                            ui.vertical(|ui| {
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new("Filter 2").strong());
                                    if ui.checkbox(&mut patch.page_a.filter2_cutoff_link, "Link").changed() {
                                        changes.push((0x70, 45, if patch.page_a.filter2_cutoff_link { 1 } else { 0 }));
                                    }
                                });

                                ComboBox::from_id_source("f2_mode")
                                    .selected_text(format!("{:?}", patch.page_a.filter2_mode))
                                    .show_ui(ui, |ui| {
                                        for mode in [FilterMode::Lowpass, FilterMode::Highpass, FilterMode::Bandpass, FilterMode::Bandstop] {
                                            if ui.selectable_value(&mut patch.page_a.filter2_mode, mode, format!("{:?}", mode)).changed() {
                                                changes.push((0x70, 40, patch.page_a.filter2_mode.to_raw()));
                                            }
                                        }
                                    });

                                ui.horizontal(|ui| {
                                    if unipolar_knob(ui, &mut patch.page_a.filter2_cutoff, "Cutoff").changed() {
                                        changes.push((0x70, 41, patch.page_a.filter2_cutoff.raw()));
                                    }
                                    if unipolar_knob(ui, &mut patch.page_a.filter2_resonance, "Resonance").changed() {
                                        changes.push((0x70, 42, patch.page_a.filter2_resonance.raw()));
                                    }
                                    if bipolar_knob(ui, &mut patch.page_a.filter2_env_amount, "Env Amt").changed() {
                                        changes.push((0x70, 43, patch.page_a.filter2_env_amount.to_raw()));
                                    }
                                    if bipolar_knob(ui, &mut patch.page_a.filter2_keyfollow, "Keyfollow").changed() {
                                        changes.push((0x70, 44, patch.page_a.filter2_keyfollow.to_raw()));
                                    }
                                });
                            });
                        });

                        ui.add_space(8.0);

                        // Routing & Balance
                        Frame::canvas(ui.style()).show(ui, |ui| {
                            ui.vertical(|ui| {
                                ui.label(RichText::new("Routing & Balance").strong());
                                ComboBox::from_id_source("filter_routing")
                                    .selected_text(format!("{:?}", patch.page_a.filter_routing))
                                    .show_ui(ui, |ui| {
                                        for routing in [FilterRouting::Serial4, FilterRouting::Serial6, FilterRouting::Parallel, FilterRouting::Split] {
                                            if ui.selectable_value(&mut patch.page_a.filter_routing, routing, format!("{:?}", routing)).changed() {
                                                changes.push((0x70, 47, patch.page_a.filter_routing.to_raw()));
                                            }
                                        }
                                    });

                                ui.horizontal(|ui| {
                                    if bipolar_knob(ui, &mut patch.page_a.filter_balance, "Balance").changed() {
                                        changes.push((0x70, 46, patch.page_a.filter_balance.to_raw()));
                                    }
                                });
                            });
                        });

                        ui.add_space(8.0);

                        // Frequency response visualizer
                        ui.add(FilterCurveWidget::new(
                            patch.page_a.filter1_cutoff,
                            patch.page_a.filter1_resonance,
                            patch.page_a.filter1_mode,
                            patch.page_a.filter2_cutoff,
                            patch.page_a.filter2_resonance,
                            patch.page_a.filter2_mode,
                            patch.page_a.filter_routing,
                        ));
                    });
                });

                ui.add_space(8.0);

                // --- 3. Envelopes & Visualizers ---
                Frame::group(ui.style()).show(ui, |ui| {
                    ui.heading(RichText::new("3. Envelopes").color(Color32::from_rgb(240, 200, 140)));
                    ui.add_space(4.0);

                    ui.horizontal_wrapped(|ui| {
                        // Filter Envelope
                        Frame::canvas(ui.style()).show(ui, |ui| {
                            ui.vertical(|ui| {
                                ui.label(RichText::new("Filter Envelope").strong());
                                ui.horizontal(|ui| {
                                    if unipolar_knob(ui, &mut patch.page_a.filter_attack, "Attack").changed() {
                                        changes.push((0x70, 48, patch.page_a.filter_attack.raw()));
                                    }
                                    if unipolar_knob(ui, &mut patch.page_a.filter_decay, "Decay").changed() {
                                        changes.push((0x70, 49, patch.page_a.filter_decay.raw()));
                                    }
                                    if unipolar_knob(ui, &mut patch.page_a.filter_sustain, "Sustain").changed() {
                                        changes.push((0x70, 50, patch.page_a.filter_sustain.raw()));
                                    }
                                    if bipolar_knob(ui, &mut patch.page_a.filter_time, "Time").changed() {
                                        changes.push((0x70, 51, patch.page_a.filter_time.to_raw()));
                                    }
                                    if unipolar_knob(ui, &mut patch.page_a.filter_release, "Release").changed() {
                                        changes.push((0x70, 52, patch.page_a.filter_release.raw()));
                                    }
                                });
                            });
                        });

                        ui.add_space(4.0);

                        ui.add(EnvelopeCurveWidget::new(
                            "Filter Envelope",
                            patch.page_a.filter_attack,
                            patch.page_a.filter_decay,
                            patch.page_a.filter_sustain,
                            patch.page_a.filter_release,
                        ));

                        ui.add_space(12.0);

                        // Amp Envelope
                        Frame::canvas(ui.style()).show(ui, |ui| {
                            ui.vertical(|ui| {
                                ui.label(RichText::new("Amplifier Envelope").strong());
                                ui.horizontal(|ui| {
                                    if unipolar_knob(ui, &mut patch.page_a.amp_attack, "Attack").changed() {
                                        changes.push((0x70, 56, patch.page_a.amp_attack.raw()));
                                    }
                                    if unipolar_knob(ui, &mut patch.page_a.amp_decay, "Decay").changed() {
                                        changes.push((0x70, 57, patch.page_a.amp_decay.raw()));
                                    }
                                    if unipolar_knob(ui, &mut patch.page_a.amp_sustain, "Sustain").changed() {
                                        changes.push((0x70, 58, patch.page_a.amp_sustain.raw()));
                                    }
                                    if bipolar_knob(ui, &mut patch.page_a.amp_time, "Time").changed() {
                                        changes.push((0x70, 59, patch.page_a.amp_time.to_raw()));
                                    }
                                    if unipolar_knob(ui, &mut patch.page_a.amp_release, "Release").changed() {
                                        changes.push((0x70, 60, patch.page_a.amp_release.raw()));
                                    }
                                });
                            });
                        });

                        ui.add_space(4.0);

                        ui.add(EnvelopeCurveWidget::new(
                            "Amplifier Envelope",
                            patch.page_a.amp_attack,
                            patch.page_a.amp_decay,
                            patch.page_a.amp_sustain,
                            patch.page_a.amp_release,
                        ));
                    });
                });

                ui.add_space(8.0);

                // --- 4. LFOs ---
                Frame::group(ui.style()).show(ui, |ui| {
                    ui.heading(RichText::new("4. Low Frequency Oscillators").color(Color32::from_rgb(240, 200, 140)));
                    ui.add_space(4.0);

                    ui.horizontal_wrapped(|ui| {
                        // LFO 1
                        Frame::canvas(ui.style()).show(ui, |ui| {
                            ui.vertical(|ui| {
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new("LFO 1").strong());
                                    if ui.checkbox(&mut patch.page_a.lfo1_keyfollow, "Keyfollow").changed() {
                                        changes.push((0x70, 67, if patch.page_a.lfo1_keyfollow { 1 } else { 0 }));
                                    }
                                });

                                ui.horizontal(|ui| {
                                    ComboBox::from_id_source("lfo1_shape")
                                        .selected_text(format!("{:?}", patch.page_a.lfo1_shape))
                                        .show_ui(ui, |ui| {
                                            for shape in [LfoShape::Sine, LfoShape::Triangle, LfoShape::Saw, LfoShape::Square, LfoShape::SampleAndHold, LfoShape::SampleAndGlide] {
                                                if ui.selectable_value(&mut patch.page_a.lfo1_shape, shape, format!("{:?}", shape)).changed() {
                                                    changes.push((0x70, 64, patch.page_a.lfo1_shape.to_raw()));
                                                }
                                            }
                                        });

                                    ComboBox::from_id_source("lfo1_trig")
                                        .selected_text(format!("{:?}", patch.page_a.lfo1_trigger))
                                        .show_ui(ui, |ui| {
                                            for trig in [LfoTriggerMode::Poly, LfoTriggerMode::Mono] {
                                                if ui.selectable_value(&mut patch.page_a.lfo1_trigger, trig, format!("{:?}", trig)).changed() {
                                                    changes.push((0x70, 68, patch.page_a.lfo1_trigger.to_raw()));
                                                }
                                            }
                                        });
                                });

                                ui.horizontal(|ui| {
                                    if unipolar_knob(ui, &mut patch.page_a.lfo1_rate, "Rate").changed() {
                                        changes.push((0x70, 65, patch.page_a.lfo1_rate.raw()));
                                    }
                                    if bipolar_knob(ui, &mut patch.page_a.lfo1_contour, "Contour").changed() {
                                        changes.push((0x70, 66, patch.page_a.lfo1_contour.to_raw()));
                                    }
                                });
                            });
                        });

                        ui.add_space(8.0);

                        // LFO 2
                        Frame::canvas(ui.style()).show(ui, |ui| {
                            ui.vertical(|ui| {
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new("LFO 2").strong());
                                    if ui.checkbox(&mut patch.page_a.lfo2_keyfollow, "Keyfollow").changed() {
                                        changes.push((0x70, 75, if patch.page_a.lfo2_keyfollow { 1 } else { 0 }));
                                    }
                                });

                                ui.horizontal(|ui| {
                                    ComboBox::from_id_source("lfo2_shape")
                                        .selected_text(format!("{:?}", patch.page_a.lfo2_shape))
                                        .show_ui(ui, |ui| {
                                            for shape in [LfoShape::Sine, LfoShape::Triangle, LfoShape::Saw, LfoShape::Square, LfoShape::SampleAndHold, LfoShape::SampleAndGlide] {
                                                if ui.selectable_value(&mut patch.page_a.lfo2_shape, shape, format!("{:?}", shape)).changed() {
                                                    changes.push((0x70, 72, patch.page_a.lfo2_shape.to_raw()));
                                                }
                                            }
                                        });

                                    ComboBox::from_id_source("lfo2_trig")
                                        .selected_text(format!("{:?}", patch.page_a.lfo2_trigger))
                                        .show_ui(ui, |ui| {
                                            for trig in [LfoTriggerMode::Poly, LfoTriggerMode::Mono] {
                                                if ui.selectable_value(&mut patch.page_a.lfo2_trigger, trig, format!("{:?}", trig)).changed() {
                                                    changes.push((0x70, 76, patch.page_a.lfo2_trigger.to_raw()));
                                                }
                                            }
                                        });
                                });

                                ui.horizontal(|ui| {
                                    if unipolar_knob(ui, &mut patch.page_a.lfo2_rate, "Rate").changed() {
                                        changes.push((0x70, 73, patch.page_a.lfo2_rate.raw()));
                                    }
                                    if bipolar_knob(ui, &mut patch.page_a.lfo2_contour, "Contour").changed() {
                                        changes.push((0x70, 74, patch.page_a.lfo2_contour.to_raw()));
                                    }
                                });
                            });
                        });
                    });
                });

                ui.add_space(8.0);

                // --- 5. 3-Band EQ & FX (Page B) ---
                Frame::group(ui.style()).show(ui, |ui| {
                    ui.heading(RichText::new("5. 3-Band Equalizer & Effects").color(Color32::from_rgb(240, 200, 140)));
                    ui.add_space(4.0);

                    ui.horizontal_wrapped(|ui| {
                        // 3-Band EQ
                        Frame::canvas(ui.style()).show(ui, |ui| {
                            ui.vertical(|ui| {
                                ui.label(RichText::new("3-Band EQ").strong());
                                ui.horizontal(|ui| {
                                    if bipolar_knob(ui, &mut patch.page_b.eq_low_gain, "Low Gain").changed() {
                                        changes.push((0x71, 64, patch.page_b.eq_low_gain.to_raw()));
                                    }
                                    if unipolar_knob(ui, &mut patch.page_b.eq_low_freq, "Low Freq").changed() {
                                        changes.push((0x71, 65, patch.page_b.eq_low_freq.raw()));
                                    }
                                    if bipolar_knob(ui, &mut patch.page_b.eq_mid_gain, "Mid Gain").changed() {
                                        changes.push((0x71, 68, patch.page_b.eq_mid_gain.to_raw()));
                                    }
                                    if unipolar_knob(ui, &mut patch.page_b.eq_mid_freq, "Mid Freq").changed() {
                                        changes.push((0x71, 69, patch.page_b.eq_mid_freq.raw()));
                                    }
                                    if unipolar_knob(ui, &mut patch.page_b.eq_mid_q, "Mid Q").changed() {
                                        changes.push((0x71, 70, patch.page_b.eq_mid_q.raw()));
                                    }
                                    if bipolar_knob(ui, &mut patch.page_b.eq_high_gain, "High Gain").changed() {
                                        changes.push((0x71, 66, patch.page_b.eq_high_gain.to_raw()));
                                    }
                                    if unipolar_knob(ui, &mut patch.page_b.eq_high_freq, "High Freq").changed() {
                                        changes.push((0x71, 67, patch.page_b.eq_high_freq.raw()));
                                    }
                                });
                            });
                        });

                        ui.add_space(8.0);

                        // Phaser
                        Frame::canvas(ui.style()).show(ui, |ui| {
                            ui.vertical(|ui| {
                                ui.label(RichText::new("Phaser").strong());
                                ui.horizontal(|ui| {
                                    if unipolar_knob(ui, &mut patch.page_b.phaser_rate, "Rate").changed() {
                                        changes.push((0x71, 49, patch.page_b.phaser_rate.raw()));
                                    }
                                    if unipolar_knob(ui, &mut patch.page_b.phaser_depth, "Depth").changed() {
                                        changes.push((0x71, 50, patch.page_b.phaser_depth.raw()));
                                    }
                                    if bipolar_knob(ui, &mut patch.page_b.phaser_feedback, "Feedback").changed() {
                                        changes.push((0x71, 51, patch.page_b.phaser_feedback.to_raw()));
                                    }
                                });
                            });
                        });

                        ui.add_space(8.0);

                        // Analog Boost
                        Frame::canvas(ui.style()).show(ui, |ui| {
                            ui.vertical(|ui| {
                                ui.label(RichText::new("Analog Boost").strong());
                                ui.horizontal(|ui| {
                                    if unipolar_knob(ui, &mut patch.page_b.boost_intensity, "Intensity").changed() {
                                        changes.push((0x71, 72, patch.page_b.boost_intensity.raw()));
                                    }
                                    if unipolar_knob(ui, &mut patch.page_b.boost_tune, "Tune").changed() {
                                        changes.push((0x71, 73, patch.page_b.boost_tune.raw()));
                                    }
                                });
                            });
                        });
                    });
                });
            });

        changes
    }
}
