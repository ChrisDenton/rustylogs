// Taken from my reading of ECMA-35 and ECMA-48

const BEL: u8 = 0x7;
const ESC: u8 = 0x1B;
const CSI: u8 = b'[';
const OSC: u8 = b']';

// Quick state machine.
// This could be optimized.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AnsiMode {
    Text,
    Escape,
    Osc,
    /// `OscEscape` on its own is ambiguous.
    /// If it's followed by OscTerminator then it ends the Osc text.
    /// If not then it's part of the Osc text.
    OscEscape,
    OscTerminator,
    Parameter,
    Intermidiate,
    Final,
}
impl AnsiMode {
    pub fn update(&mut self, b: u8) -> Self {
        *self = self.next(b);
        *self
    }

    pub fn next(self, b: u8) -> Self {
        match (self, b) {
            (Self::Text, ESC) => Self::Escape,
            (Self::Escape, CSI) => Self::Parameter,
            (Self::Escape, OSC) => Self::Osc,
            (Self::Escape, 0x20..=0x2f) => Self::Intermidiate,
            (Self::Escape, 0x40..=0x7e) => Self::Final,
            (Self::Parameter, 0x30..=0x3f) => Self::Parameter,
            (Self::Parameter, 0x20..=0x2f) => Self::Intermidiate,
            (Self::Parameter, 0x40..=0x7e) => Self::Final,
            (Self::Intermidiate, 0x20..=0x2f) => Self::Intermidiate,
            (Self::Intermidiate, 0x40..=0x7e) => Self::Final,
            // Handle Operating System Commands
            (Self::Osc, ESC) => Self::OscEscape,
            (Self::OscEscape, b'\\') => Self::OscTerminator,
            (Self::OscEscape, ESC) => Self::OscEscape,
            (Self::Osc | Self::OscEscape, BEL) => Self::OscTerminator,
            (Self::Osc | Self::OscEscape, _) => Self::Osc,
            (Self::Final | Self::OscTerminator, ESC) => Self::Escape,
            // Anything else is just text
            _ => Self::Text,
        }
    }

    pub fn is_text(self) -> bool {
        self == Self::Text
    }
}
