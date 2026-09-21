use oqueue_codec::apikey::ApiKey;

pub(super) fn alter_configs_body(out: &mut Vec<u8>, version: i16) {
    use oqueue_codec::flex::{TaggedFields, put_array_len, put_string, put_tagged_fields};
    use oqueue_codec::wire::{put_bool, put_i8};
    let flexible = version >= 2;
    put_array_len(out, flexible, Some(1));
    put_i8(out, 2);
    put_string(out, flexible, "t");
    put_array_len(out, flexible, Some(0));
    if flexible {
        put_tagged_fields(out, &TaggedFields::default());
    }
    if version >= 1 {
        put_bool(out, false);
    }
    if flexible {
        put_tagged_fields(out, &TaggedFields::default());
    }
}

pub(super) fn incremental_alter_configs_body(out: &mut Vec<u8>, version: i16) {
    use oqueue_codec::flex::{
        TaggedFields, put_array_len, put_nullable_string, put_string, put_tagged_fields,
    };
    use oqueue_codec::wire::{put_bool, put_i8};
    let flexible = version >= 1;
    put_array_len(out, flexible, Some(1));
    put_i8(out, 2);
    put_string(out, flexible, "t");
    put_array_len(out, flexible, Some(1));
    put_string(out, flexible, "retention.ms");
    put_i8(out, 1);
    put_nullable_string(out, flexible, None);
    if flexible {
        put_tagged_fields(out, &TaggedFields::default());
        put_tagged_fields(out, &TaggedFields::default());
    }
    put_bool(out, false);
    if flexible {
        put_tagged_fields(out, &TaggedFields::default());
    }
}

pub(super) fn config_error_code(api_key: ApiKey, rest: &mut &[u8], version: i16) -> i16 {
    use oqueue_codec::flex::{
        read_array_len, read_nullable_string, read_string, read_tagged_fields,
    };
    use oqueue_codec::wire::Cursor;
    let input = *rest;
    let mut cur = Cursor::new(input);
    let _ = cur.read_i32().expect("config throttle decodes");
    let flexible = match api_key {
        ApiKey::AlterConfigs => version >= 2,
        ApiKey::IncrementalAlterConfigs => version >= 1,
        _ => false,
    };
    let count = read_array_len(&mut cur, flexible)
        .expect("config response count decodes")
        .expect("config response count is not null");
    let mut error = 0;
    for index in 0..count {
        let code = cur.read_i16().expect("config error decodes");
        let _ = read_nullable_string(&mut cur, flexible).expect("config message decodes");
        let _ = cur.read_i8().expect("config type decodes");
        let _ = read_string(&mut cur, flexible).expect("config name decodes");
        if flexible {
            read_tagged_fields(&mut cur).expect("config fields decode");
        }
        if index == 0 {
            error = code;
        }
    }
    if flexible {
        read_tagged_fields(&mut cur).expect("config response fields decode");
    }
    *rest = &input[input.len() - cur.remaining()..];
    error
}
