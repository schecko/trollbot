
use std::borrow::Cow;
use std::collections::HashMap;

#[derive(Debug)]
pub enum MapValue<'a> {
    FileName(&'a str),
    Value(&'a str),
}

#[derive(Debug)]
pub struct MultiTrigger<'a> {
    pub triggers: [&'a str; 4],
    pub value: MapValue<'a>, 
}

// TODO split parsing with substitution
pub fn subst_global<'a>(message: Cow<'a, str>) -> Cow<'a, str> {
    if message.contains("{") {
        let result = message.replace("{me}", "somewhatinaccurate"); // TODO get this from somewhere
        return Cow::Owned(result);
    } else {
        return message;
    } 
}

// limitation: keys generated from values that contain capitals will never be tolowered, so those
// keys will always fail to compare
pub fn load_map<'a>(contents: &'a str, lists: &HashMap<&'a str, Vec<&'a str>>) -> (Vec<MultiTrigger<'a>>, HashMap<Cow<'a, str>, MapValue<'a>>) {
    let mut map = HashMap::new();
    let mut multi_triggers = Vec::new();
    for line in contents.lines() {
        let mut split = line.split('='); 
        if let (Some(meta_key), Some(value)) = (split.next(), split.next()) {
            if meta_key.len() == 0 { continue; }
            if value.len() == 0 { continue; }
            if let Some('-') = meta_key.chars().next() {
                continue;
            }

            // the starting character can be a meta key, if the meta_key is a forward square
            // bracket, then the key is pointing to a list file. treat each entry as a key
            let single = vec![meta_key];
            let keys = if let Some('[') = meta_key.chars().next() {
                lists.get(&meta_key[1..]).unwrap()
            } else {
                &single
            };

            'key_loop: for key in keys { 
                let map_value = if let Some('[') = value.chars().next() {
                    MapValue::FileName(&value[1..]) 
                } else {
                    MapValue::Value(value) 
                };

                if key.contains(' ') {
                    let mut multi_split = key.split(' ');

                    let first = multi_split.next();
                    let second = multi_split.next();
                    if first == None { continue 'key_loop; }
                    if second == None { continue 'key_loop; }

                    multi_triggers.push(MultiTrigger { 
                        triggers: [
                            first.unwrap(),
                            second.unwrap(),
                            multi_split.next().unwrap_or(""),
                            multi_split.next().unwrap_or(""),
                        ],
                        value: map_value,
                    }); 
                } else {
                    if key.contains('{') { continue 'key_loop; }
                    map.insert(subst_global(Cow::Borrowed(*key)), map_value);
                }
            }
        }
    } 
    (multi_triggers, map)
}
