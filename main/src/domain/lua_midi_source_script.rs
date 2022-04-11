use crate::domain::SafeLua;
use helgoboss_learn::{
    AbsoluteValue, FeedbackValue, MidiSourceScript, RawMidiEvent, RawMidiEvents,
};
use mlua::{ChunkMode, Function, Table, ToLua, Value};
use std::error::Error;

#[derive(Clone, Debug)]
pub struct LuaMidiSourceScript<'lua> {
    lua: &'lua SafeLua,
    function: Function<'lua>,
    env: Table<'lua>,
    y_key: Value<'lua>,
}

unsafe impl<'a> Send for LuaMidiSourceScript<'a> {}

impl<'lua> LuaMidiSourceScript<'lua> {
    pub fn compile(lua: &'lua SafeLua, lua_script: &str) -> Result<Self, Box<dyn Error>> {
        if lua_script.trim().is_empty() {
            return Err("script empty".into());
        }
        let env = lua.create_fresh_environment()?;
        let chunk = lua
            .as_ref()
            .load(lua_script)
            .set_name("MIDI source script")?
            .set_environment(env.clone())?
            .set_mode(ChunkMode::Text);
        let function = chunk.into_function()?;
        let script = Self {
            lua,
            env,
            function,
            y_key: "y".to_lua(lua.as_ref())?,
        };
        Ok(script)
    }
}

impl<'a> MidiSourceScript for LuaMidiSourceScript<'a> {
    fn execute(&self, input_value: FeedbackValue) -> Result<RawMidiEvents, &'static str> {
        // TODO-high We don't limit the time of each execution at the moment because not sure
        //  how expensive this measurement is. But it would actually be useful to do it for MIDI
        //  scripts!
        let y_value = match input_value {
            FeedbackValue::Off => Value::Nil,
            FeedbackValue::Numeric(n) => match n.value {
                AbsoluteValue::Continuous(v) => Value::Number(v.get()),
                AbsoluteValue::Discrete(f) => Value::Integer(f.actual() as _),
            },
            FeedbackValue::Textual(v) => v
                .text
                .to_lua(self.lua.as_ref())
                .map_err(|_| "couldn't convert string to Lua string")?,
        };
        self.env
            .raw_set(self.y_key.clone(), y_value)
            .map_err(|_| "couldn't set y variable")?;
        let messages: Vec<Vec<u8>> = self
            .function
            .call(())
            .map_err(|_| "failed to invoke Lua script")?;
        let events = messages
            .into_iter()
            .flat_map(|msg| RawMidiEvent::try_from_slice(0, &msg))
            .collect();
        Ok(events)
    }
}
