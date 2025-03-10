/*
 * Copyright (c) 2022 Yunshan Networks
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

package metrics

import (
	"errors"
	"fmt"
	"strings"

	ckcommon "github.com/deepflowys/deepflow/server/querier/engine/clickhouse/common"

	logging "github.com/op/go-logging"
)

var log = logging.MustGetLogger("clickhouse.metrics")

const METRICS_OPERATOR_GTE = ">="
const METRICS_OPERATOR_LTE = "<="

var METRICS_OPERATORS = []string{METRICS_OPERATOR_GTE, METRICS_OPERATOR_LTE}

type Metrics struct {
	Index       int    // 索引
	DBField     string // 数据库字段
	DisplayName string // 描述
	Unit        string // 单位
	Type        int    // 指标量类型
	Category    string // 类别
	Condition   string // 聚合过滤
	IsAgg       bool   // 是否为聚合指标量
	Permissions []bool // 指标量的权限控制
	Table       string // 所属表
}

func (m *Metrics) Replace(metrics *Metrics) {
	m.IsAgg = metrics.IsAgg
	if metrics.DBField != "" {
		m.DBField = metrics.DBField
	}
	if metrics.Condition != "" {
		m.Condition = metrics.Condition
	}
}

func (m *Metrics) SetIsAgg(isAgg bool) *Metrics {
	m.IsAgg = isAgg
	return m
}

func NewMetrics(
	index int, dbField string, displayname string, unit string, metricType int, category string,
	permissions []bool, condition string, table string,
) *Metrics {
	return &Metrics{
		Index:       index,
		DBField:     dbField,
		DisplayName: displayname,
		Unit:        unit,
		Type:        metricType,
		Category:    category,
		Permissions: permissions,
		Condition:   condition,
		Table:       table,
	}
}

func NewReplaceMetrics(dbField string, condition string) *Metrics {
	return &Metrics{
		DBField:   dbField,
		Condition: condition,
		IsAgg:     true,
	}
}

func GetMetrics(field string, db string, table string) (*Metrics, bool) {
	if db == "ext_metrics" || db == "deepflow_system" {
		field = strings.Trim(field, "`")
		fieldSplit := strings.Split(field, ".")
		if len(fieldSplit) > 1 {
			if fieldSplit[0] == "metrics" {
				return NewMetrics(
					0, fmt.Sprintf("if(indexOf(metrics_float_names, '%s')=0,null,metrics_float_values[indexOf(metrics_float_names, '%s')])", fieldSplit[1], fieldSplit[1]),
					field, "", METRICS_TYPE_COUNTER,
					"指标", []bool{true, true, true}, "", table,
				), true
			}
		}
	}
	allMetrics, err := GetMetricsByDBTable(db, table, "")
	if err != nil {
		return nil, false
	}
	metric, ok := allMetrics[field]
	return metric, ok
}

func GetMetricsByDBTable(db string, table string, where string) (map[string]*Metrics, error) {
	var err error
	switch db {
	case "flow_log":
		switch table {
		case "l4_flow_log":
			return GetL4FlowLogMetrics(), err
		case "l7_flow_log":
			return GetL7FlowLogMetrics(), err
		}
	case "flow_metrics":
		switch table {
		case "vtap_flow_port":
			return GetVtapFlowPortMetrics(), err
		case "vtap_flow_edge_port":
			return GetVtapFlowEdgePortMetrics(), err
		case "vtap_app_port":
			return GetVtapAppPortMetrics(), err
		case "vtap_app_edge_port":
			return GetVtapAppEdgePortMetrics(), err
		case "vtap_acl":
			return GetVtapAclMetrics(), err
		}
	case "ext_metrics", "deepflow_system":
		return GetExtMetrics(db, table, where)
	}
	return nil, err
}

func GetMetricsDescriptionsByDBTable(db string, table string, where string) ([]interface{}, error) {
	allMetrics, err := GetMetricsByDBTable(db, table, where)
	if allMetrics == nil || err != nil {
		// TODO: metrics not found
		return nil, err
	}
	/* columns := []interface{}{
		"name", "is_agg", "display_name", "unit", "type", "category", "operators", "permissions", "table"
	} */
	values := make([]interface{}, len(allMetrics))
	for field, metrics := range allMetrics {
		values[metrics.Index] = []interface{}{
			field, metrics.IsAgg, metrics.DisplayName, metrics.Unit, metrics.Type,
			metrics.Category, METRICS_OPERATORS, metrics.Permissions, metrics.Table,
		}
	}
	return values, nil
}

func GetMetricsDescriptions(db string, table string, where string) (map[string][]interface{}, error) {
	var values []interface{}
	if table == "" {
		var tables []interface{}
		if db == "ext_metrics" || db == "deepflow_system" {
			for _, extTables := range ckcommon.GetExtTables(db) {
				for i, extTable := range extTables.([]interface{}) {
					if i == 0 {
						tables = append(tables, extTable)
					}
				}
			}
		} else {
			for _, dbTable := range ckcommon.DB_TABLE_MAP[db] {
				tables = append(tables, dbTable)
			}
		}
		for _, dbTable := range tables {
			metrics, err := GetMetricsDescriptionsByDBTable(db, dbTable.(string), where)
			if err != nil {
				return nil, err
			}
			values = append(values, metrics...)
		}
	} else {
		metrics, err := GetMetricsDescriptionsByDBTable(db, table, where)
		if err != nil {
			return nil, err
		}
		values = append(values, metrics...)
	}
	columns := []interface{}{
		"name", "is_agg", "display_name", "unit", "type", "category", "operators", "permissions", "table",
	}
	return map[string][]interface{}{
		"columns": columns,
		"values":  values,
	}, nil
}

func LoadMetrics(db string, table string, dbDescription map[string]interface{}) (loadMetrics map[string]*Metrics, err error) {
	tableDate, ok := dbDescription[db]
	if !ok {
		return nil, errors.New(fmt.Sprintf("get metrics failed! db: %s", db))
	}
	if ok {
		metricsData, ok := tableDate.(map[string]interface{})[table]
		if ok {
			loadMetrics = make(map[string]*Metrics)
			for i, metrics := range metricsData.([][]interface{}) {
				if len(metrics) < 7 {
					return nil, errors.New(fmt.Sprintf("get metrics failed! db:%s table:%s metrics:%v", db, table, metrics))
				}
				metricType, ok := METRICS_TYPE_NAME_MAP[metrics[4].(string)]
				if !ok {
					return nil, errors.New(fmt.Sprintf("get metrics type failed! db:%s table:%s metrics:%v", db, table, metrics))
				}
				permissions, err := ckcommon.ParsePermission(metrics[6])
				if err != nil {
					return nil, errors.New(fmt.Sprintf("parse metrics permission failed! db:%s table:%s metrics:%v", db, table, metrics))
				}
				lm := NewMetrics(
					i, metrics[1].(string), metrics[2].(string), metrics[3].(string), metricType,
					metrics[5].(string), permissions, "", table,
				)
				loadMetrics[metrics[0].(string)] = lm
			}
		} else {
			return nil, errors.New(fmt.Sprintf("get metrics failed! db:%s table:%s", db, table))
		}
	}
	return loadMetrics, nil
}

func MergeMetrics(db string, table string, loadMetrics map[string]*Metrics) error {
	var metrics map[string]*Metrics
	var replaceMetrics map[string]*Metrics
	switch db {
	case "flow_log":
		switch table {
		case "l4_flow_log":
			metrics = L4_FLOW_LOG_METRICS
			replaceMetrics = L4_FLOW_LOG_METRICS_REPLACE
		case "l7_flow_log":
			metrics = L7_FLOW_LOG_METRICS
			replaceMetrics = L7_FLOW_LOG_METRICS_REPLACE
		}
	case "flow_metrics":
		switch table {
		case "vtap_flow_port":
			metrics = VTAP_FLOW_PORT_METRICS
			replaceMetrics = VTAP_FLOW_PORT_METRICS_REPLACE
		case "vtap_flow_edge_port":
			metrics = VTAP_FLOW_EDGE_PORT_METRICS
			replaceMetrics = VTAP_FLOW_EDGE_PORT_METRICS_REPLACE
		case "vtap_app_port":
			metrics = VTAP_APP_PORT_METRICS
			replaceMetrics = VTAP_APP_PORT_METRICS_REPLACE
		case "vtap_app_edge_port":
			metrics = VTAP_APP_EDGE_PORT_METRICS
			replaceMetrics = VTAP_APP_EDGE_PORT_METRICS_REPLACE
		case "vtap_acl":
			metrics = VTAP_ACL_METRICS
			replaceMetrics = VTAP_ACL_METRICS_REPLACE
		}
	case "ext_metrics", "deepflow_system":
		metrics = EXT_METRICS
	}
	if metrics == nil {
		return errors.New(fmt.Sprintf("merge metrics failed! db:%s, table:%s", db, table))
	}
	for name, value := range loadMetrics {
		// TAG类型指标量都属于聚合类型
		if value.Type == METRICS_TYPE_TAG {
			value.IsAgg = true
		}
		if rm, ok := replaceMetrics[name]; ok && value.DBField == "" {
			value.Replace(rm)
		}
		metrics[name] = value
	}
	return nil
}
