use std::collections::{HashMap, HashSet};
use std::io;

use ad_stream_protocol::{InputMessage, TouchPhase};
use evdev::uinput::VirtualDevice;
use evdev::{
    AbsInfo, AbsoluteAxisCode, AttributeSet, EventType, InputEvent, KeyCode, PropType,
    RelativeAxisCode, UinputAbsSetup,
};
use thiserror::Error;

const KEY_MAX: u16 = 0x2ff;
const BTN_LEFT: u8 = 1;
const BTN_RIGHT: u8 = 2;
const BTN_MIDDLE: u8 = 3;
const MOD_SHIFT: u16 = 1 << 0;
const MOD_CONTROL: u16 = 1 << 1;
const MOD_ALT: u16 = 1 << 2;
const MOD_META: u16 = 1 << 3;

#[derive(Clone, Debug)]
pub struct InputConfig {
    pub width: i32,
    pub height: i32,
    pub max_touch_points: u8,
    /// Pointer acceleration in the inclusive range `0.0..=1.0`.
    /// Zero preserves the client's absolute coordinates exactly.
    pub pointer_acceleration: f64,
}

impl Default for InputConfig {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            max_touch_points: 10,
            pointer_acceleration: 0.0,
        }
    }
}

impl InputConfig {
    pub fn validate(&self) -> Result<(), InputError> {
        if self.width < 1 || self.height < 1 {
            return Err(InputError::InvalidConfiguration(
                "input dimensions must be positive",
            ));
        }
        if self.max_touch_points == 0 || self.max_touch_points > 32 {
            return Err(InputError::InvalidConfiguration(
                "max_touch_points must be between 1 and 32",
            ));
        }
        if !self.pointer_acceleration.is_finite()
            || !(0.0..=1.0).contains(&self.pointer_acceleration)
        {
            return Err(InputError::InvalidConfiguration(
                "pointer_acceleration must be between 0.0 and 1.0",
            ));
        }
        Ok(())
    }
}

pub struct InputEngine {
    keyboard: VirtualDevice,
    pointer: VirtualDevice,
    touch: VirtualDevice,
    config: InputConfig,
    modifiers: u16,
    pointer_position: (i32, i32),
    touch_slots: HashMap<u32, i32>,
    free_touch_slots: Vec<i32>,
    pressed_keys: HashSet<u16>,
}

impl InputEngine {
    pub fn new(config: InputConfig) -> Result<Self, InputError> {
        config.validate()?;

        // Probe through the uinput crate as well as evdev so failures point at
        // the kernel device/ACL before partially creating virtual devices.
        uinput::open("/dev/uinput").map_err(|error| InputError::Unavailable(error.to_string()))?;

        let keyboard = create_keyboard()?;
        let pointer = create_pointer(&config)?;
        let touch = create_touch(&config)?;
        let free_touch_slots = (0..i32::from(config.max_touch_points)).rev().collect();

        Ok(Self {
            keyboard,
            pointer,
            touch,
            config,
            modifiers: 0,
            pointer_position: (0, 0),
            touch_slots: HashMap::new(),
            free_touch_slots,
            pressed_keys: HashSet::new(),
        })
    }

    pub fn handle(&mut self, message: &InputMessage) -> Result<(), InputError> {
        match message {
            InputMessage::Keyboard {
                scan_code,
                pressed,
                modifiers,
                repeat,
            } => self.keyboard(*scan_code, *pressed, *modifiers, *repeat),
            InputMessage::PointerMove { x, y, .. } => self.pointer_move(*x, *y),
            InputMessage::PointerButton {
                button,
                pressed,
                x,
                y,
            } => {
                self.pointer_move(*x, *y)?;
                self.pointer_button(*button, *pressed)
            }
            InputMessage::PointerWheel {
                horizontal,
                vertical,
            } => self.pointer_wheel(*horizontal, *vertical),
            InputMessage::Touch {
                contact_id,
                phase,
                x,
                y,
                pressure,
            } => self.touch(*contact_id, *phase, *x, *y, *pressure),
            InputMessage::UnicodeText { .. } => Err(InputError::UnicodeRequiresTextInput),
        }
    }

    pub fn release_all(&mut self) -> Result<(), InputError> {
        let releases: Vec<_> = self
            .pressed_keys
            .drain()
            .map(|code| InputEvent::new(EventType::KEY.0, code, 0))
            .collect();
        if !releases.is_empty() {
            self.keyboard.emit(&releases)?;
        }
        self.modifiers = 0;

        let contacts: Vec<_> = self.touch_slots.keys().copied().collect();
        for id in contacts {
            self.touch(id, TouchPhase::Cancel, 0, 0, 0)?;
        }
        Ok(())
    }

    fn keyboard(
        &mut self,
        scan_code: u32,
        pressed: bool,
        modifiers: u16,
        repeat: bool,
    ) -> Result<(), InputError> {
        let code = u16::try_from(scan_code).map_err(|_| InputError::InvalidKey(scan_code))?;
        if code == 0 || code > KEY_MAX {
            return Err(InputError::InvalidKey(scan_code));
        }
        self.synchronize_modifiers(modifiers)?;
        let value = if repeat {
            2
        } else if pressed {
            1
        } else {
            0
        };
        self.keyboard
            .emit(&[InputEvent::new(EventType::KEY.0, code, value)])?;
        if pressed {
            self.pressed_keys.insert(code);
        } else {
            self.pressed_keys.remove(&code);
        }
        Ok(())
    }

    fn synchronize_modifiers(&mut self, requested: u16) -> Result<(), InputError> {
        let mapping = [
            (MOD_SHIFT, KeyCode::KEY_LEFTSHIFT.0),
            (MOD_CONTROL, KeyCode::KEY_LEFTCTRL.0),
            (MOD_ALT, KeyCode::KEY_LEFTALT.0),
            (MOD_META, KeyCode::KEY_LEFTMETA.0),
        ];
        let mut events = Vec::with_capacity(mapping.len());
        for (bit, code) in mapping {
            let was_pressed = self.modifiers & bit != 0;
            let should_press = requested & bit != 0;
            if was_pressed != should_press {
                events.push(InputEvent::new(
                    EventType::KEY.0,
                    code,
                    i32::from(should_press),
                ));
                if should_press {
                    self.pressed_keys.insert(code);
                } else {
                    self.pressed_keys.remove(&code);
                }
            }
        }
        if !events.is_empty() {
            self.keyboard.emit(&events)?;
        }
        self.modifiers = requested & (MOD_SHIFT | MOD_CONTROL | MOD_ALT | MOD_META);
        Ok(())
    }

    fn pointer_move(&mut self, x: i32, y: i32) -> Result<(), InputError> {
        let position = accelerated_position(
            self.pointer_position,
            (x, y),
            self.config.pointer_acceleration,
            (self.config.width, self.config.height),
        );
        self.pointer.emit(&[
            InputEvent::new(EventType::ABSOLUTE.0, AbsoluteAxisCode::ABS_X.0, position.0),
            InputEvent::new(EventType::ABSOLUTE.0, AbsoluteAxisCode::ABS_Y.0, position.1),
        ])?;
        self.pointer_position = position;
        Ok(())
    }

    fn pointer_button(&mut self, button: u8, pressed: bool) -> Result<(), InputError> {
        let key = match button {
            BTN_LEFT => KeyCode::BTN_LEFT,
            BTN_RIGHT => KeyCode::BTN_RIGHT,
            BTN_MIDDLE => KeyCode::BTN_MIDDLE,
            4 => KeyCode::BTN_SIDE,
            5 => KeyCode::BTN_EXTRA,
            _ => return Err(InputError::InvalidPointerButton(button)),
        };
        self.pointer
            .emit(&[InputEvent::new(EventType::KEY.0, key.0, i32::from(pressed))])?;
        Ok(())
    }

    fn pointer_wheel(&mut self, horizontal: i32, vertical: i32) -> Result<(), InputError> {
        let mut events = Vec::with_capacity(2);
        if horizontal != 0 {
            events.push(InputEvent::new(
                EventType::RELATIVE.0,
                RelativeAxisCode::REL_HWHEEL_HI_RES.0,
                horizontal,
            ));
        }
        if vertical != 0 {
            events.push(InputEvent::new(
                EventType::RELATIVE.0,
                RelativeAxisCode::REL_WHEEL_HI_RES.0,
                vertical,
            ));
        }
        if !events.is_empty() {
            self.pointer.emit(&events)?;
        }
        Ok(())
    }

    fn touch(
        &mut self,
        contact_id: u32,
        phase: TouchPhase,
        x: i32,
        y: i32,
        pressure: u16,
    ) -> Result<(), InputError> {
        if matches!(phase, TouchPhase::Begin) && self.touch_slots.contains_key(&contact_id) {
            return Err(InputError::DuplicateTouch(contact_id));
        }

        let slot = match phase {
            TouchPhase::Begin => {
                let slot = self
                    .free_touch_slots
                    .pop()
                    .ok_or(InputError::TooManyTouches)?;
                self.touch_slots.insert(contact_id, slot);
                slot
            }
            _ => *self
                .touch_slots
                .get(&contact_id)
                .ok_or(InputError::UnknownTouch(contact_id))?,
        };

        let mut events = vec![InputEvent::new(
            EventType::ABSOLUTE.0,
            AbsoluteAxisCode::ABS_MT_SLOT.0,
            slot,
        )];
        match phase {
            TouchPhase::Begin => {
                events.push(InputEvent::new(
                    EventType::ABSOLUTE.0,
                    AbsoluteAxisCode::ABS_MT_TRACKING_ID.0,
                    i32::try_from(contact_id).unwrap_or(i32::MAX),
                ));
                events.push(InputEvent::new(EventType::KEY.0, KeyCode::BTN_TOUCH.0, 1));
            }
            TouchPhase::Update => {}
            TouchPhase::End | TouchPhase::Cancel => events.push(InputEvent::new(
                EventType::ABSOLUTE.0,
                AbsoluteAxisCode::ABS_MT_TRACKING_ID.0,
                -1,
            )),
        }
        if !matches!(phase, TouchPhase::End | TouchPhase::Cancel) {
            events.extend([
                InputEvent::new(
                    EventType::ABSOLUTE.0,
                    AbsoluteAxisCode::ABS_MT_POSITION_X.0,
                    x.clamp(0, self.config.width - 1),
                ),
                InputEvent::new(
                    EventType::ABSOLUTE.0,
                    AbsoluteAxisCode::ABS_MT_POSITION_Y.0,
                    y.clamp(0, self.config.height - 1),
                ),
                InputEvent::new(
                    EventType::ABSOLUTE.0,
                    AbsoluteAxisCode::ABS_MT_PRESSURE.0,
                    i32::from(pressure),
                ),
            ]);
        }
        if matches!(phase, TouchPhase::End | TouchPhase::Cancel) {
            self.touch_slots.remove(&contact_id);
            self.free_touch_slots.push(slot);
            if self.touch_slots.is_empty() {
                events.push(InputEvent::new(EventType::KEY.0, KeyCode::BTN_TOUCH.0, 0));
            }
        }
        self.touch.emit(&events)?;
        Ok(())
    }
}

impl Drop for InputEngine {
    fn drop(&mut self) {
        let _ = self.release_all();
    }
}

fn create_keyboard() -> io::Result<VirtualDevice> {
    let mut keys = AttributeSet::<KeyCode>::new();
    for code in 1..=KEY_MAX {
        keys.insert(KeyCode(code));
    }
    VirtualDevice::builder()?
        .name("AetherDesk ARPP Keyboard")
        .with_keys(&keys)?
        .build()
}

fn create_pointer(config: &InputConfig) -> io::Result<VirtualDevice> {
    let mut keys = AttributeSet::<KeyCode>::new();
    for key in [
        KeyCode::BTN_LEFT,
        KeyCode::BTN_RIGHT,
        KeyCode::BTN_MIDDLE,
        KeyCode::BTN_SIDE,
        KeyCode::BTN_EXTRA,
    ] {
        keys.insert(key);
    }
    let mut relative = AttributeSet::<RelativeAxisCode>::new();
    relative.insert(RelativeAxisCode::REL_WHEEL);
    relative.insert(RelativeAxisCode::REL_HWHEEL);
    relative.insert(RelativeAxisCode::REL_WHEEL_HI_RES);
    relative.insert(RelativeAxisCode::REL_HWHEEL_HI_RES);

    VirtualDevice::builder()?
        .name("AetherDesk ARPP Pointer")
        .with_keys(&keys)?
        .with_relative_axes(&relative)?
        .with_absolute_axis(&absolute_axis(AbsoluteAxisCode::ABS_X, config.width - 1))?
        .with_absolute_axis(&absolute_axis(AbsoluteAxisCode::ABS_Y, config.height - 1))?
        .build()
}

fn create_touch(config: &InputConfig) -> io::Result<VirtualDevice> {
    let mut keys = AttributeSet::<KeyCode>::new();
    keys.insert(KeyCode::BTN_TOUCH);
    let mut properties = AttributeSet::<PropType>::new();
    properties.insert(PropType::DIRECT);

    VirtualDevice::builder()?
        .name("AetherDesk ARPP Touchscreen")
        .with_keys(&keys)?
        .with_properties(&properties)?
        .with_absolute_axis(&absolute_axis(
            AbsoluteAxisCode::ABS_MT_SLOT,
            i32::from(config.max_touch_points) - 1,
        ))?
        .with_absolute_axis(&absolute_axis(
            AbsoluteAxisCode::ABS_MT_TRACKING_ID,
            i32::MAX,
        ))?
        .with_absolute_axis(&absolute_axis(
            AbsoluteAxisCode::ABS_MT_POSITION_X,
            config.width - 1,
        ))?
        .with_absolute_axis(&absolute_axis(
            AbsoluteAxisCode::ABS_MT_POSITION_Y,
            config.height - 1,
        ))?
        .with_absolute_axis(&absolute_axis(
            AbsoluteAxisCode::ABS_MT_PRESSURE,
            i32::from(u16::MAX),
        ))?
        .build()
}

fn absolute_axis(code: AbsoluteAxisCode, maximum: i32) -> UinputAbsSetup {
    UinputAbsSetup::new(code, AbsInfo::new(0, 0, maximum, 0, 0, 1))
}

fn accelerated_position(
    previous: (i32, i32),
    requested: (i32, i32),
    acceleration: f64,
    bounds: (i32, i32),
) -> (i32, i32) {
    if acceleration == 0.0 {
        return (
            requested.0.clamp(0, bounds.0 - 1),
            requested.1.clamp(0, bounds.1 - 1),
        );
    }
    let dx = f64::from(requested.0 - previous.0);
    let dy = f64::from(requested.1 - previous.1);
    let speed = dx.hypot(dy);
    let gain = 1.0 + acceleration * (speed / 100.0).min(1.0);
    (
        (f64::from(previous.0) + dx * gain)
            .round()
            .clamp(0.0, f64::from(bounds.0 - 1)) as i32,
        (f64::from(previous.1) + dy * gain)
            .round()
            .clamp(0.0, f64::from(bounds.1 - 1)) as i32,
    )
}

#[derive(Debug, Error)]
pub enum InputError {
    #[error("uinput is unavailable: {0}; grant this user read/write access to /dev/uinput")]
    Unavailable(String),
    #[error("invalid input configuration: {0}")]
    InvalidConfiguration(&'static str),
    #[error("invalid keyboard scan code {0}")]
    InvalidKey(u32),
    #[error("invalid pointer button {0}")]
    InvalidPointerButton(u8),
    #[error("touch contact {0} already exists")]
    DuplicateTouch(u32),
    #[error("touch contact {0} does not exist")]
    UnknownTouch(u32),
    #[error("the negotiated touch-contact limit was exceeded")]
    TooManyTouches,
    #[error("Unicode text must use the Wayland text-input protocol instead of raw evdev")]
    UnicodeRequiresTextInput,
    #[error("uinput I/O failed: {0}")]
    Io(#[from] io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_configuration_without_opening_uinput() {
        let invalid = InputConfig {
            width: 0,
            ..InputConfig::default()
        };
        assert!(matches!(
            invalid.validate(),
            Err(InputError::InvalidConfiguration(_))
        ));
    }

    #[test]
    fn acceleration_is_bounded_and_can_be_disabled() {
        assert_eq!(
            accelerated_position((10, 10), (20, 30), 0.0, (100, 100)),
            (20, 30)
        );
        let accelerated = accelerated_position((90, 90), (120, 120), 1.0, (100, 100));
        assert_eq!(accelerated, (99, 99));
    }
}
