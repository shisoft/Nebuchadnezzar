(ns neb.test.nested
  (:require [midje.sweet :refer :all]
            [neb.schema :refer [add-schema]]
            [neb.cell :refer [new-cell read-cell delete-cell replace-cell update-cell get-in-cell select-keys-from-cell]]
            [cluster-connector.utils.for-debug :refer [$]])
  (:import (org.shisoft.neb Trunk)))

(fact "Test Internal Array"
      (let [trunk (Trunk. (Trunk/getSegSize) 0)]
        (fact "Array Schema"
              (add-schema :array-schema [[:arr :long-array]] 1) => anything)
        (fact "Write Cell With Array"
              (new-cell trunk 1 1 (int 1) {:arr (range 100)}) => anything)
        (fact "Read Cell With Array"
              (read-cell trunk 1) => (contains {:arr (vec (range 100))}))
        (.dispose trunk)))

(fact "Test Array"
      (let [trunk (Trunk. (Trunk/getSegSize) 0)]
        (fact "Array Schema"
              (add-schema :array-schema [[:arr [:ARRAY :long]]] 1) => anything)
        (fact "Write Cell With Array"
              (new-cell trunk 1 1 (int 1) {:arr (range 100)}) => anything)
        (fact "Read Cell With Array"
              (read-cell trunk 1) => (contains {:arr (vec (range 100))}))
        (.dispose trunk)))

(fact "Test Nested Array"
      (let [trunk (Trunk. (Trunk/getSegSize) 0)]
        (fact "Array Schema"
              (add-schema :array-schema [[:arr [:ARRAY [:ARRAY :long]]]] 1) => anything)
        (fact "Write Cell With Array"
              (new-cell trunk 1 1 (int 1) {:arr (repeat 100 (range 100))}) => anything)
        (fact "Read Cell With Array"
              (read-cell trunk 1) => (contains {:arr (vec (repeat 100 (vec (range 100))))}))
        (.dispose trunk)))

(fact "Test Map"
      (let [trunk (Trunk. (Trunk/getSegSize) 0)]
        (fact "Map Schema"
              (add-schema :array-schema [[:map [[:a :long] [:b :long]]]] 1) => anything)
        (fact "Write Cell With Map"
              (new-cell trunk 1 1 (int 1) {:map {:a 1 :b 2}}) => anything)
        (fact "Read Cell With Map"
              (read-cell trunk 1) => (contains {:map {:a 1 :b 2}}))
        (.dispose trunk)))

(fact "Test Map Array"
      (let [trunk (Trunk. (Trunk/getSegSize) 0)]
        (fact "Map Schema"
              (add-schema :array-schema [[:map [[:a :long] [:b [:ARRAY :long]]]]] 1) => anything)
        (fact "Write Cell With Map"
              (new-cell trunk 1 1 (int 1) {:map {:a 1 :b (range 1000)}}) => anything)
        (fact "Read Cell With Map"
              (read-cell trunk 1) => (contains {:map {:a 1 :b (range 1000)}}))
        (.dispose trunk)))

(fact "Test Array Map"
      (let [trunk (Trunk. (Trunk/getSegSize) 0)]
        (fact "Map Schema"
              (add-schema :array-schema [[:map [[:a :long] [:b [:ARRAY [[:arr-map :long]]]]]]] 1) => anything)
        (fact "Write Cell With Map"
              (new-cell trunk 1 1 (int 1) {:map {:a 1 :b (repeat 1000 {:arr-map 50})}}) => anything)
        (fact "Read Cell With Map"
              (read-cell trunk 1) => (contains {:map {:a 1 :b (repeat 1000 {:arr-map 50})}}))
        (.dispose trunk)))

(fact "Test Schema Type"
      (let [trunk (Trunk. (Trunk/getSegSize) 0)]
        (fact "Schemas"
              (add-schema :item-schema [[:id :long] [:val :long]] 2) => anything
              (add-schema :array-schema [[:data :item-schema]] 1) => anything)
        (fact "Write Cell With Schema Type"
              (new-cell trunk 1 1 (int 1) {:data {:id 1 :val 2}}) => anything)
        (fact "Read Cell With Schema Type"
              (read-cell trunk 1) => (contains {:data {:id 1 :val 2}}))
        (.dispose trunk)))

(fact "Test Schema Type in array"
      (let [trunk (Trunk. (Trunk/getSegSize) 0)]
        (fact "Schemas"
              (add-schema :item-schema [[:id :long] [:val :long]] 2) => anything
              (add-schema :array-schema [[:data [:ARRAY :item-schema]]] 1) => anything)
        (fact "Write Cell With Schema Type"
              (new-cell trunk 1 1 (int 1) {:data (repeat 1000 {:id 1 :val 2})}) => anything)
        (fact "Read Cell With Schema Type"
              (read-cell trunk 1) => (contains {:data (repeat 1000 {:id 1 :val 2})}))
        (.dispose trunk)))

(fact "Test get-in and select-keys"
      (let [trunk (Trunk. (Trunk/getSegSize) 0)]
        (fact "Map Schema"
              (add-schema :array-schema [[:a :int]
                                         [:b :int]
                                         [:c :int]
                                         [:map [[:a :long] [:b [:ARRAY [[:arr-map :long]]]]]]] 1) => anything)
        (fact "Write Cell With Map"
              (new-cell trunk 1 1 (int 1) {:map {:a 1 :b (repeat 1000 {:arr-map 50})}
                                         :a 1
                                         :b 2
                                         :c 3}) => anything)
        (fact "Read Cell With Map"
              (read-cell trunk 1) => (contains {:map {:a 1 :b (repeat 1000 {:arr-map 50})}}))
        (fact "get-in"
              (get-in-cell trunk 1 [:map :a]) => 1
              (get-in-cell trunk 1 :map) => {:a 1 :b (repeat 1000 {:arr-map 50})}
              (get-in-cell trunk 1 [:map :b]) => (repeat 1000 {:arr-map 50})
              (get-in-cell trunk 1 [:map :b 0 :arr-map]) => 50)
        (fact "select-keys"
              (select-keys-from-cell trunk 1 [:a :c]) => {:a 1 :c 3}
              (select-keys-from-cell trunk 1 [:a :b]) => {:a 1 :b 2})
        (.dispose trunk)))