package config

// LabelCreatedBy is the conventional label for the embedding application.
const LabelCreatedBy = "created-by"

// WithCreatedBy returns a copy of svc with labels["created-by"]=who.
// Existing labels are preserved; created-by is set/overwritten.
func WithCreatedBy(svc ServiceDef, who string) ServiceDef {
	out := svc
	if out.Labels == nil {
		out.Labels = map[string]string{}
	} else {
		cp := make(map[string]string, len(out.Labels)+1)
		for k, v := range out.Labels {
			cp[k] = v
		}
		out.Labels = cp
	}
	out.Labels[LabelCreatedBy] = who
	return out
}

// MatchLabels reports whether have contains every key=value in want (AND).
// An empty want always matches.
func MatchLabels(have, want map[string]string) bool {
	for k, v := range want {
		if have[k] != v {
			return false
		}
	}
	return true
}
