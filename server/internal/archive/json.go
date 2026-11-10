package archive

import "encoding/json"

func marshalJSON(value any) ([]byte, error)      { return json.MarshalIndent(value, "", "  ") }
func unmarshalJSON(data []byte, value any) error { return json.Unmarshal(data, value) }
